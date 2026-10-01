//! 一時VM・サンドボックスノードのプロビジョナー管理 (中央サーバーホスト側)。
//!
//! 設計: `docs/01-architecture-and-sync.md` §6.1〜§6.4、
//! `docs/05-cli-and-pwa-ui.md` §2.2。
//!
//! - `config.toml` の `[provisioners.<name>]` に定義されたコマンドを子プロセスとして
//!   起動し、`stdin` / `stdout` を `NodeToServerMsg` / `ServerToNodeMsg` の
//!   JSON Lines トランスポートとして使用する (`fxg daemon --stdio` と直結)
//! - プロビジョナー起動時に `FXG_GIT_URL` / `FXG_GIT_BRANCH` / 短命 `FXG_GIT_TOKEN`
//!   と一時ノードID (`FXG_NODE_ID`) を環境変数として注入する
//! - `stderr` は `BootstrapLog` として UI へストリーム配信する (エフェメラル)
//! - ハンドシェイク (NodeHello) 受信後、ハブ接続を登録してから `StartSession` を
//!   送信してセッションを開始する (ブートストラップ中はノードは「オフライン」)
//! - アイドルタイムアウト / 全セッション終了時に `DrainAndShutdown` を送り、
//!   `DrainComplete` 後に子プロセス (VM/コンテナ) を破棄する
//! - `WorkspaceBundleUpload` は `DrainComplete` までチャンクを蓄積し、
//!   `~/.flexagent/bundles/<session-id>.bundle` へ保存して
//!   `sessions.git_bundle_path` に記録する

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use fxg_protocol::client_api::ProvisionerTestResponse;
use fxg_protocol::common::{ForkHistoryItem, NodeLifecycleStatus, SessionStatus};
use fxg_protocol::config::{
    BUNDLES_DIR_NAME, DEFAULT_GIT_USERNAME, GitCredentialConfig, ProvisionerConfig,
};
use fxg_protocol::node_server::ServerToNodeMsg;
use fxg_protocol::util::uuid_v7;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::Command;
use tokio::sync::{oneshot, watch};

use crate::api::{ApiError, ClientEvent};
use crate::credentials::resolve_credential;
use crate::hub;
use crate::state::ServerState;

/// ブートストラップ完了 (NodeHello) を待つ上限。
const HELLO_TIMEOUT: Duration = Duration::from_secs(1800);
/// `DrainComplete` を待つ上限。
const DRAIN_TIMEOUT: Duration = Duration::from_secs(60);
/// アイドル監視のティック間隔。
const MONITOR_INTERVAL: Duration = Duration::from_secs(5);
/// `provisioners test` のハンドシェイク待ち上限。
const TEST_TIMEOUT: Duration = Duration::from_secs(300);
/// `provisioners test` で保持するブートストラップログ行数。
const TEST_LOG_LINES: usize = 200;
/// 一時VM内のワークスペースパス (生成スクリプトと一致させる)。
pub(crate) const EPHEMERAL_WORKSPACE: &str = "/tmp/workspace";
/// 一時VM内の fxg バイナリパス。
const EPHEMERAL_BINARY: &str = "/tmp/fxg";

/// 一時VMインスタンスの実行状態 (node_id をキーに管理)。
struct Instance {
    node_id: String,
    session_id: Option<String>,
    /// 子プロセス終了シグナル (pump タスクが待機し、kill + Job Object 破棄を行う)
    terminate: watch::Sender<bool>,
    /// 最終メッセージ受信時刻 (アイドルタイムアウト判定)
    last_activity: Arc<StdMutex<Instant>>,
}

/// 蓄積中の Git バンドル (session_id → Base64 チャンク列)。
#[derive(Default)]
struct BundleAccumulator {
    chunks: Vec<String>,
    branch: String,
    head_commit: String,
}

struct ProvisionerInner {
    fxg_home: PathBuf,
    configs: BTreeMap<String, ProvisionerConfig>,
    git_credentials: BTreeMap<String, GitCredentialConfig>,
    instances: StdMutex<HashMap<String, Instance>>,
    bundles: StdMutex<HashMap<String, BundleAccumulator>>,
    drain_waiters: StdMutex<HashMap<String, oneshot::Sender<()>>>,
}

/// 一時VMプロビジョナーの管理 (中央サーバーが 1 つ保持する)。
#[derive(Clone)]
pub struct ProvisionerManager {
    inner: Arc<ProvisionerInner>,
}

impl std::fmt::Debug for ProvisionerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let instances = self
            .inner
            .instances
            .lock()
            .map(|instances| instances.len())
            .unwrap_or(0);
        f.debug_struct("ProvisionerManager")
            .field("provisioners", &self.inner.configs.len())
            .field("instances", &instances)
            .finish()
    }
}

impl ProvisionerManager {
    /// 設定から管理を作成する。
    pub fn new(
        fxg_home: PathBuf,
        configs: BTreeMap<String, ProvisionerConfig>,
        git_credentials: BTreeMap<String, GitCredentialConfig>,
    ) -> Self {
        Self {
            inner: Arc::new(ProvisionerInner {
                fxg_home,
                configs,
                git_credentials,
                instances: StdMutex::new(HashMap::new()),
                bundles: StdMutex::new(HashMap::new()),
                drain_waiters: StdMutex::new(HashMap::new()),
            }),
        }
    }

    /// 定義済みプロビジョナー設定。
    pub fn configs(&self) -> &BTreeMap<String, ProvisionerConfig> {
        &self.inner.configs
    }

    /// 設定を取り出す (未知の名前は `NOT_FOUND`)。
    pub fn config(&self, name: &str) -> Result<&ProvisionerConfig, ApiError> {
        self.inner
            .configs
            .get(name)
            .ok_or_else(|| ApiError::not_found(format!("provisioner not found: {name}")))
    }

    /// Git Credential Proxy 設定。
    pub fn git_credentials(&self) -> &BTreeMap<String, GitCredentialConfig> {
        &self.inner.git_credentials
    }

    /// `~/.flexagent` データディレクトリ。
    pub fn fxg_home(&self) -> &std::path::Path {
        &self.inner.fxg_home
    }

    /// 稼働中の一時ノードか (稼働中インスタンスが存在するか)。
    pub fn is_ephemeral(&self, node_id: &str) -> bool {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .contains_key(node_id)
    }

    /// セッションID (未知のノードは `None`)。
    fn session_id(&self, node_id: &str) -> Option<String> {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .get(node_id)
            .and_then(|instance| instance.session_id.clone())
    }

    fn activity(&self, node_id: &str) -> Option<Arc<StdMutex<Instant>>> {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .get(node_id)
            .map(|instance| instance.last_activity.clone())
    }

    fn insert_instance(&self, instance: Instance) {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .insert(instance.node_id.clone(), instance);
    }

    fn remove_instance(&self, node_id: &str) {
        self.inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .remove(node_id);
    }

    /// 子プロセスへ終了 (kill + Job Object 破棄) を要求する。
    fn terminate(&self, node_id: &str) {
        if let Some(instance) = self
            .inner
            .instances
            .lock()
            .expect("instances lock poisoned")
            .get(node_id)
        {
            let _ = instance.terminate.send(true);
        }
    }

    fn register_drain_waiter(&self, node_id: &str) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .drain_waiters
            .lock()
            .expect("drain lock poisoned")
            .insert(node_id.to_owned(), tx);
        rx
    }

    /// `DrainComplete` を受信した (待機中があれば通知)。
    fn take_drain_waiter(&self, node_id: &str) {
        if let Some(tx) = self
            .inner
            .drain_waiters
            .lock()
            .expect("drain lock poisoned")
            .remove(node_id)
        {
            let _ = tx.send(());
        }
    }

    fn push_bundle_chunk(&self, session_id: &str, branch: &str, head_commit: &str, chunk: &str) {
        let mut bundles = self.inner.bundles.lock().expect("bundles lock poisoned");
        let entry = bundles.entry(session_id.to_owned()).or_default();
        if !branch.is_empty() {
            entry.branch = branch.to_owned();
        }
        if !head_commit.is_empty() {
            entry.head_commit = head_commit.to_owned();
        }
        entry.chunks.push(chunk.to_owned());
    }

    fn take_bundle(&self, session_id: &str) -> Option<BundleAccumulator> {
        self.inner
            .bundles
            .lock()
            .expect("bundles lock poisoned")
            .remove(session_id)
    }
}

/// セッション開始要求 (プロビジョナー起動 + StartSession)。
#[derive(Debug, Clone)]
pub struct SpawnSessionRequest {
    /// サーバー採番のセッションID
    pub session_id: String,
    /// 一時ノードID (サーバー採番。`FXG_NODE_ID` として注入)
    pub node_id: String,
    /// プロビジョナー名
    pub provisioner: String,
    /// 論理プロジェクトID
    pub project_id: String,
    /// クローン元 Git URL (`FXG_GIT_URL`)
    pub git_url: String,
    /// クローン対象ブランチ (`FXG_GIT_BRANCH`。`--worktree` 指定時に設定)
    pub git_branch: Option<String>,
    /// 起動するエージェントID
    pub agent_id: String,
    /// 初期プロンプト
    pub initial_prompt: Option<String>,
    /// 別ノードからの Fork 履歴 (Replay 注入)
    pub fork_context: Option<Vec<ForkHistoryItem>>,
    /// 退避済み Git バンドル (Base64)
    pub restore_bundle_b64: Option<String>,
}

/// セッション用のプロビジョナーを起動する (ハンドシェイク後に StartSession を送る)。
pub async fn spawn_session(
    state: &ServerState,
    request: SpawnSessionRequest,
) -> Result<(), ApiError> {
    let config = state.provisioners().config(&request.provisioner)?.clone();
    let node_id = request.node_id.clone();
    let session_id = request.session_id.clone();
    let idle_timeout_secs = config.idle_timeout_secs.unwrap_or(900);

    let mut env: Vec<(String, String)> = vec![
        ("FXG_NODE_ID".to_owned(), node_id.clone()),
        (
            "FXG_NODE_NAME".to_owned(),
            format!("{} (ephemeral)", request.provisioner),
        ),
        ("FXG_GIT_URL".to_owned(), request.git_url.clone()),
    ];
    if let Some(branch) = request.git_branch.as_deref().filter(|b| !b.is_empty()) {
        env.push(("FXG_GIT_BRANCH".to_owned(), branch.to_owned()));
    }
    match resolve_git_token(state, &request.git_url).await {
        Some(response) => {
            env.push((
                "FXG_GIT_USERNAME".to_owned(),
                if response.username.trim().is_empty() {
                    DEFAULT_GIT_USERNAME.to_owned()
                } else {
                    response.username
                },
            ));
            if let Some(token) = response.token {
                env.push(("FXG_GIT_TOKEN".to_owned(), token));
            }
        }
        None => {
            tracing::warn!(
                git_url = request.git_url,
                "no git credential configured for the project host; clone may fail"
            );
        }
    }

    let pending = PendingStart {
        session_id: session_id.clone(),
        project_id: request.project_id.clone(),
        agent_id: request.agent_id.clone(),
        initial_prompt: request.initial_prompt.clone(),
        fork_context: request.fork_context.clone(),
        restore_bundle_b64: request.restore_bundle_b64.clone(),
    };

    let spawn = SpawnRequest {
        node_id: node_id.clone(),
        provisioner: request.provisioner.clone(),
        config,
        env,
        mode: SpawnMode::Session(Box::new(pending.clone())),
    };
    let handles = spawn_child(state, spawn).await?;
    let hello = handles
        .hello
        .ok_or_else(|| ApiError::internal("ephemeral spawn returned no handshake channel"))?;

    tokio::spawn(await_handshake_and_start(
        state.clone(),
        node_id.clone(),
        hello,
        pending,
    ));
    tokio::spawn(monitor_instance(
        state.clone(),
        node_id.clone(),
        Duration::from_secs(idle_timeout_secs),
    ));
    tracing::info!(
        node_id,
        session_id,
        provisioner = request.provisioner,
        "ephemeral node provisioning started"
    );
    Ok(())
}

/// 一時ノードを事前登録する (`sessions.node_id` の FK と `provisioner` 表示のため)。
///
/// `NodeHello` の upsert は `provisioner = NULL` を送るが、`COALESCE` により
/// ここで登録した値が保持される。
pub(crate) async fn register_ephemeral_node(
    state: &ServerState,
    node_id: &str,
    provisioner: &str,
    idle_timeout_secs: u64,
) -> Result<(), ApiError> {
    let mut record = fxg_db::NodeRecord::new(
        node_id.to_owned(),
        format!("{provisioner} (ephemeral)"),
        "linux",
        "x86_64",
        env!("CARGO_PKG_VERSION"),
    );
    record.is_ephemeral = true;
    record.provisioner = Some(provisioner.to_owned());
    record.lifecycle_status = NodeLifecycleStatus::Provisioning;
    record.idle_timeout_secs = Some(idle_timeout_secs);
    record.is_online = false;
    state
        .db()
        .upsert_node(&record)
        .await
        .map_err(ApiError::from)
}

/// `fxg provisioners test <name>`: プロビジョナーの起動と NodeHello 疎通を検証する。
pub async fn provisioner_test(
    state: &ServerState,
    name: &str,
) -> Result<ProvisionerTestResponse, ApiError> {
    let config = state.provisioners().config(name)?.clone();
    let node_id = format!("fxg-test-{}", &uuid_v7()[..8]);

    let spawn = SpawnRequest {
        node_id: node_id.clone(),
        provisioner: name.to_owned(),
        config,
        env: vec![("FXG_NODE_ID".to_owned(), node_id.clone())],
        mode: SpawnMode::Test,
    };
    let handles = spawn_child(state, spawn).await?;
    let hello = handles
        .hello
        .ok_or_else(|| ApiError::internal("provisioner test returned no handshake channel"))?;
    let terminate = handles.terminate.clone();
    let test_log = handles.test_log.clone().unwrap_or_default();

    let outcome = tokio::time::timeout(TEST_TIMEOUT, hello).await;
    let log_lines: Vec<String> = test_log
        .lock()
        .expect("test log poisoned")
        .iter()
        .rev()
        .take(TEST_LOG_LINES)
        .rev()
        .cloned()
        .collect();

    // 検証後は必ず破棄する (Drain はベストエフォート)
    let _ = state
        .hub()
        .send(
            &node_id,
            ServerToNodeMsg::DrainAndShutdown {
                reason: "provisioner test completed".to_owned(),
                create_git_bundle: false,
            },
        )
        .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = terminate.send(true);
    state.hub().detach_ephemeral(&node_id, None).await;
    if let Err(err) = state
        .db()
        .set_node_lifecycle(&node_id, NodeLifecycleStatus::Terminated)
        .await
    {
        tracing::debug!(node_id, "failed to mark test node terminated: {err}");
    }

    match outcome {
        Ok(Ok(hello_node_id)) => Ok(ProvisionerTestResponse {
            name: name.to_owned(),
            ok: true,
            node_id: Some(hello_node_id),
            log_lines,
            error: None,
        }),
        Ok(Err(_)) => Ok(ProvisionerTestResponse {
            name: name.to_owned(),
            ok: false,
            node_id: None,
            log_lines,
            error: Some("provisioner exited before the daemon handshake".to_owned()),
        }),
        Err(_) => Ok(ProvisionerTestResponse {
            name: name.to_owned(),
            ok: false,
            node_id: None,
            log_lines,
            error: Some(format!(
                "timed out waiting for NodeHello ({}s)",
                TEST_TIMEOUT.as_secs()
            )),
        }),
    }
}

/// 子プロセス起動要求。
struct SpawnRequest {
    node_id: String,
    provisioner: String,
    config: ProvisionerConfig,
    env: Vec<(String, String)>,
    mode: SpawnMode,
}

/// 起動モード (セッション用 / 疎通検証用)。
enum SpawnMode {
    /// セッション用: ブートストラップ後に `StartSession` を送る
    Session(Box<PendingStart>),
    /// 疎通検証用: NodeHello を待って終了する
    Test,
}

/// `StartSession` 送信に必要な情報 (ハンドシェイク後に使用)。
#[derive(Clone)]
struct PendingStart {
    session_id: String,
    project_id: String,
    agent_id: String,
    initial_prompt: Option<String>,
    fork_context: Option<Vec<ForkHistoryItem>>,
    restore_bundle_b64: Option<String>,
}

/// 子プロセス起動後のハンドル。
struct SpawnHandles {
    /// NodeHello 受信時にノードIDを通知する (失敗時はチャネルクローズ)
    hello: Option<oneshot::Receiver<String>>,
    /// 子プロセス終了シグナル
    terminate: watch::Sender<bool>,
    /// `provisioners test` の収集ログ
    test_log: Option<Arc<StdMutex<Vec<String>>>>,
}

/// プロビジョナーコマンドを起動し、stdin/stdout/stderr のポンプを開始する。
async fn spawn_child(state: &ServerState, request: SpawnRequest) -> Result<SpawnHandles, ApiError> {
    let manager = state.provisioners().clone();
    let bootstrap = compose_bootstrap_script(
        env!("CARGO_PKG_VERSION"),
        matches!(request.mode, SpawnMode::Session(_)),
    );
    let instance_name = format!(
        "fxg-eph-{}",
        &request.node_id[..request.node_id.len().min(24)]
    );

    let mut command = Command::new(&request.config.command);
    for arg in &request.config.args {
        command.arg(
            arg.replace("{BOOTSTRAP_SCRIPT}", &bootstrap)
                .replace("{INSTANCE_NAME}", &instance_name)
                .replace("{SESSION_ID}", &request.node_id),
        );
    }
    for (key, value) in &request.config.env {
        command.env(key, value);
    }
    for (key, value) in &request.env {
        command.env(key, value);
    }
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|err| {
        ApiError::internal(format!(
            "failed to spawn provisioner {}: {err}",
            request.provisioner
        ))
    })?;

    // Windows: Job Object へ割当て、サーバー終了時に VM/コンテナを確実に終了させる
    let guard = match fxg_pty::ProcessTreeGuard::new() {
        Ok(guard) => {
            #[cfg(windows)]
            if let Some(handle) = child.raw_handle() {
                let _ = guard.attach_raw_handle(handle);
            }
            Some(guard)
        }
        Err(err) => {
            tracing::warn!("failed to create process tree guard: {err}");
            None
        }
    };

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ApiError::internal("failed to capture provisioner stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal("failed to capture provisioner stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal("failed to capture provisioner stderr"))?;

    let (hello_tx, hello_rx) = oneshot::channel::<String>();
    let (terminate_tx, terminate_rx) = watch::channel(false);
    let last_activity = Arc::new(StdMutex::new(Instant::now()));
    let test_log: Option<Arc<StdMutex<Vec<String>>>> = match request.mode {
        SpawnMode::Test => Some(Arc::new(StdMutex::new(Vec::new()))),
        SpawnMode::Session(_) => None,
    };

    let node_id = request.node_id.clone();
    let session_id = match &request.mode {
        SpawnMode::Session(pending) => Some(pending.session_id.clone()),
        SpawnMode::Test => None,
    };
    let pump_state = state.clone();
    let pump_manager = manager.clone();
    let pump_node_id = node_id.clone();
    let pump_log = test_log.clone();
    let pump_activity = last_activity.clone();
    let pump_session_id = session_id.clone();

    // stdout pump: 子プロセスの生存期間を所有し、NodeHello でハブ接続を登録する
    tokio::spawn(async move {
        let _guard = guard;
        let mut terminate_rx = terminate_rx;
        let mut child = child;
        let mut stdin_writer = Some(BufWriter::new(stdin));
        let mut stdin_forwarder: Option<tokio::task::JoinHandle<()>> = None;
        let mut hello_tx = Some(hello_tx);
        let mut stdout_lines = BufReader::new(stdout).lines();
        let mut stderr_lines = BufReader::new(stderr).lines();
        let mut attached = false;

        loop {
            tokio::select! {
                _ = terminate_rx.changed() => {
                    if *terminate_rx.borrow() {
                        let _ = child.start_kill();
                    }
                    break;
                }
                line = stderr_lines.next_line() => match line {
                    Ok(Some(line)) => {
                        // ブートストラップログはエフェメラルイベントとして UI へ配信する
                        if let Some(log) = &pump_log
                            && let Ok(mut lines) = log.lock()
                        {
                            lines.push(line.clone());
                        }
                        match &pump_session_id {
                            Some(session_id) => {
                                let _ = pump_state.hub().client_events().send(ClientEvent::BootstrapLog {
                                    session_id: session_id.clone(),
                                    line,
                                });
                            }
                            None => tracing::debug!(node_id = pump_node_id, "{line}"),
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        tracing::debug!(node_id = pump_node_id, "stderr read failed: {err}");
                    }
                },
                line = stdout_lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if let Ok(mut activity) = pump_activity.lock() {
                            *activity = Instant::now();
                        }
                        if line.trim().is_empty() {
                            continue;
                        }
                        let is_hello = line.contains("\"op\":\"node_hello\"");
                        if is_hello && !attached {
                            attached = true;
                            // ハブ接続を登録し、コマンド (StartSession 等) を
                            // 子プロセスの stdin へ転送する
                            let (mut commands, _conn_id) =
                                pump_state.hub().attach_ephemeral(&pump_node_id).await;
                            if let Some(mut writer) = stdin_writer.take() {
                                stdin_forwarder = Some(tokio::spawn(async move {
                                    while let Some(message) = commands.recv().await {
                                        let mut payload = match serde_json::to_vec(&message) {
                                            Ok(payload) => payload,
                                            Err(err) => {
                                                tracing::warn!(
                                                    "failed to encode node message: {err}"
                                                );
                                                continue;
                                            }
                                        };
                                        payload.push(b'\n');
                                        if writer.write_all(&payload).await.is_err()
                                            || writer.flush().await.is_err()
                                        {
                                            break;
                                        }
                                    }
                                }));
                            }
                            if let Some(tx) = hello_tx.take() {
                                let hello_node_id = extract_hello_node_id(&line).unwrap_or_default();
                                let _ = tx.send(hello_node_id);
                            }
                        }
                        if line.contains("\"op\":\"drain_complete\"") {
                            pump_manager.take_drain_waiter(&pump_node_id);
                        }
                        hub::handle_stdio_message(&pump_state, &pump_node_id, &line).await;
                    }
                    Ok(None) => break,
                    Err(err) => {
                        tracing::warn!(node_id = pump_node_id, "stdout read failed: {err}");
                        break;
                    }
                },
            }
        }

        // 子プロセスの終了を待って後処理する (kill 済み含む)
        let status = child.wait().await;
        if let Some(forwarder) = stdin_forwarder {
            forwarder.abort();
        }
        // ハンドシェイク前に終了した場合は受信側へ通知する (channel を閉じる)
        drop(hello_tx.take());
        pump_state
            .hub()
            .detach_ephemeral(&pump_node_id, status.ok().and_then(|status| status.code()))
            .await;
        pump_manager.remove_instance(&pump_node_id);
        tracing::info!(node_id = pump_node_id, "ephemeral node instance ended");
    });

    // アイドル監視用にインスタンスを登録 (セッション情報を保持)
    manager.insert_instance(Instance {
        node_id,
        session_id,
        terminate: terminate_tx.clone(),
        last_activity,
    });

    Ok(SpawnHandles {
        hello: Some(hello_rx),
        terminate: terminate_tx,
        test_log,
    })
}

/// NodeHello の JSON 行から `node_id` を取り出す (最小限のパース)。
fn extract_hello_node_id(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value
        .get("node_id")
        .and_then(|node_id| node_id.as_str())
        .map(str::to_owned)
}

/// ハンドシェイクを待って `StartSession` を送信する。
async fn await_handshake_and_start(
    state: ServerState,
    node_id: String,
    hello: oneshot::Receiver<String>,
    pending: PendingStart,
) {
    let handshake = tokio::time::timeout(HELLO_TIMEOUT, hello).await;
    let hello_node_id = match handshake {
        Ok(Ok(hello_node_id)) => hello_node_id,
        Ok(Err(_)) => {
            fail_session(
                &state,
                &pending.session_id,
                "provisioner exited before the daemon handshake",
            )
            .await;
            return;
        }
        Err(_) => {
            fail_session(
                &state,
                &pending.session_id,
                "timed out waiting for the ephemeral node handshake",
            )
            .await;
            drain(&state, &node_id, "bootstrap timeout", false).await;
            return;
        }
    };
    if !hello_node_id.is_empty() && hello_node_id != node_id {
        tracing::warn!(
            node_id,
            hello_node_id,
            "ephemeral node reported an unexpected node_id"
        );
    }

    let command_id = uuid_v7();
    let message = ServerToNodeMsg::StartSession {
        command_id: command_id.clone(),
        session_id: pending.session_id.clone(),
        project_id: pending.project_id.clone(),
        local_path: EPHEMERAL_WORKSPACE.to_owned(),
        agent_id: pending.agent_id.clone(),
        initial_prompt: pending.initial_prompt.clone(),
        fork_context_messages: pending.fork_context.clone(),
        restore_git_bundle_b64: pending.restore_bundle_b64.clone(),
    };
    match state.hub().command(&node_id, &command_id, message).await {
        Ok(result) if result.success => {
            tracing::info!(
                node_id,
                session_id = pending.session_id,
                "ephemeral session started"
            );
        }
        Ok(result) => {
            fail_session(
                &state,
                &pending.session_id,
                result.error.as_deref().unwrap_or("start session failed"),
            )
            .await;
        }
        Err(err) => {
            fail_session(&state, &pending.session_id, &err.message).await;
        }
    }
}

/// プロビジョニング中のセッションを失敗として記録する (ベストエフォート)。
async fn fail_session(state: &ServerState, session_id: &str, reason: &str) {
    tracing::warn!(session_id, reason, "ephemeral session failed");
    if let Err(err) = state
        .db()
        .set_provisional_session_status(session_id, SessionStatus::Error)
        .await
    {
        tracing::debug!(session_id, "failed to record session error: {err}");
    }
}

/// 一時ノードのアイドルタイムアウト / 全セッション終了を監視する。
async fn monitor_instance(state: ServerState, node_id: String, idle_timeout: Duration) {
    loop {
        tokio::time::sleep(MONITOR_INTERVAL).await;
        let manager = state.provisioners();
        let Some(activity) = manager.activity(&node_id) else {
            return;
        };
        let idle = activity
            .lock()
            .map(|value| value.elapsed())
            .unwrap_or_default();
        if idle >= idle_timeout {
            tracing::info!(node_id, "ephemeral node idle timeout; draining");
            drain(&state, &node_id, "idle timeout", true).await;
            return;
        }
        // セッションが終了していればアイドル扱いで破棄する
        if let Some(session_id) = manager.session_id(&node_id)
            && let Ok(Some(session)) = state.db().get_session(&session_id).await
            && matches!(
                session.status,
                SessionStatus::Stopped | SessionStatus::Error
            )
        {
            tracing::info!(node_id, session_id, "ephemeral session ended; draining");
            drain(&state, &node_id, "session ended", true).await;
            return;
        }
    }
}

/// `DrainAndShutdown` を送信し、`DrainComplete` (またはタイムアウト) 後に破棄する。
///
/// ベストエフォート: Drain 前にクラッシュした場合の未送信イベントは失われる。
pub(crate) async fn drain(
    state: &ServerState,
    node_id: &str,
    reason: &str,
    create_git_bundle: bool,
) -> bool {
    if !state.provisioners().is_ephemeral(node_id) {
        return false;
    }
    let drain_rx = state.provisioners().register_drain_waiter(node_id);
    tracing::info!(node_id, reason, "draining ephemeral node");
    let _ = state
        .hub()
        .send(
            node_id,
            ServerToNodeMsg::DrainAndShutdown {
                reason: reason.to_owned(),
                create_git_bundle,
            },
        )
        .await;
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, drain_rx).await.is_ok();
    if !drained {
        tracing::warn!(node_id, "drain timed out; forcing termination");
    }
    state.provisioners().terminate(node_id);
    drained
}

/// 稼働中の全一時VMのノードID (シャットダウン処理用)。
pub fn running_node_ids(state: &ServerState) -> Vec<String> {
    state
        .provisioners()
        .inner
        .instances
        .lock()
        .map(|instances| instances.keys().cloned().collect())
        .unwrap_or_default()
}

/// `WorkspaceBundleUpload` のチャンクを蓄積する。
pub(crate) fn on_bundle_upload(
    state: &ServerState,
    session_id: &str,
    branch: &str,
    head_commit: &str,
    chunk: &str,
) {
    state
        .provisioners()
        .push_bundle_chunk(session_id, branch, head_commit, chunk);
}

/// `DrainComplete` 受信時の処理 (bundle の保存 + Drain 待機の通知)。
pub(crate) async fn on_drain_complete(state: &ServerState, node_id: &str) {
    state.provisioners().take_drain_waiter(node_id);
    if let Some(session_id) = state.provisioners().session_id(node_id) {
        save_bundle(state, &session_id).await;
    }
}

/// 蓄積した bundle チャンクを連結・デコードしてファイルへ保存する。
async fn save_bundle(state: &ServerState, session_id: &str) {
    let Some(bundle) = state.provisioners().take_bundle(session_id) else {
        return;
    };
    let encoded: String = bundle.chunks.concat();
    let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded.as_bytes()) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(session_id, "failed to decode workspace bundle: {err}");
            return;
        }
    };
    let dir = state.provisioners().fxg_home().join(BUNDLES_DIR_NAME);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!(session_id, "failed to create bundles dir: {err}");
        return;
    }
    let path = dir.join(format!("{session_id}.bundle"));
    if let Err(err) = std::fs::write(&path, &bytes) {
        tracing::warn!(session_id, "failed to write workspace bundle: {err}");
        return;
    }
    if let Err(err) = state
        .db()
        .set_git_bundle_path(session_id, Some(&path.to_string_lossy()))
        .await
    {
        tracing::warn!(session_id, "failed to record git_bundle_path: {err}");
        return;
    }
    tracing::info!(
        session_id,
        path = %path.display(),
        bytes = bytes.len(),
        head_commit = bundle.head_commit,
        branch = bundle.branch,
        "workspace bundle stored"
    );
}

/// 稼働中の全一時VMを終了する (サーバーシャットダウン時)。
pub async fn terminate_all(state: &ServerState) {
    for node_id in running_node_ids(state) {
        let _ = state
            .hub()
            .send(
                &node_id,
                ServerToNodeMsg::DrainAndShutdown {
                    reason: "server shutting down".to_owned(),
                    create_git_bundle: false,
                },
            )
            .await;
        state.provisioners().terminate(&node_id);
    }
}

/// Git URL のホストに対応する短命トークンを解決する (見つからなければ `None`)。
pub(crate) async fn resolve_git_token(
    state: &ServerState,
    git_url: &str,
) -> Option<crate::credentials::CredentialResponse> {
    let host = url_host(git_url)?;
    let credentials = state.provisioners().git_credentials();
    let config = credentials.get(&host).or_else(|| credentials.get("*"))?;
    match resolve_credential(config).await {
        Ok(response) => Some(response),
        Err(err) => {
            tracing::warn!(host, "failed to resolve git credential: {err}");
            None
        }
    }
}

/// URL (`https://github.com/...` / `git@github.com:...`) からホスト名を抽出する。
pub(crate) fn url_host(url: &str) -> Option<String> {
    if let Some(rest) = url.split("://").nth(1) {
        let authority = rest.split('/').next()?;
        let host = authority
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(authority);
        let host = host.split(':').next()?;
        return (!host.is_empty()).then(|| host.to_owned());
    }
    // SCP 形式 (`git@github.com:owner/repo.git`)
    if let Some((_, rest)) = url.split_once('@')
        && let Some((host, _)) = rest.split_once(':')
        && !host.is_empty()
        && !host.contains('/')
    {
        return Some(host.to_owned());
    }
    None
}

/// 一時VMのブートストラップスクリプト (`{BOOTSTRAP_SCRIPT}`) を生成する。
///
/// すべての出力を `stderr` (`>&2`) に流し、`stdout` は `fxg daemon --stdio`
/// のプロトコル通信専用として保護する。
fn compose_bootstrap_script(version: &str, clone_workspace: bool) -> String {
    let clone_step = if clone_workspace {
        format!(
            r#"'{}' bootstrap-workspace --repo "$FXG_GIT_URL" ${{FXG_GIT_BRANCH:+--branch "$FXG_GIT_BRANCH"}} --dir {} >&2
if [ -f "$FXG_HOME/bootstrap.env" ]; then . "$FXG_HOME/bootstrap.env"; fi
exec '{}' daemon --stdio --ephemeral --workspace {}"#,
            EPHEMERAL_BINARY, EPHEMERAL_WORKSPACE, EPHEMERAL_BINARY, EPHEMERAL_WORKSPACE
        )
    } else {
        // 疎通検証: クローンを行わずデーモンのハンドシェイクのみ確認する
        format!(
            "exec '{}' daemon --stdio --ephemeral --workspace /tmp",
            EPHEMERAL_BINARY
        )
    };

    format!(
        r#"set -eu
export FXG_HOME="${{FXG_HOME:-$HOME/.flexagent}}"
mkdir -p "$FXG_HOME"
if ! command -v git >/dev/null 2>&1 || ! command -v curl >/dev/null 2>&1; then
  if command -v apt-get >/dev/null 2>&1; then
    (apt-get update && apt-get install -y --no-install-recommends git curl ca-certificates) >&2 || true
  fi
fi
fxg_os=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$fxg_os" in
  linux) fxg_os=linux ;;
  darwin) fxg_os=macos ;;
  *) fxg_os=linux ;;
esac
fxg_arch=$(uname -m)
case "$fxg_arch" in
  x86_64|amd64) fxg_arch=x86_64 ;;
  aarch64|arm64) fxg_arch=aarch64 ;;
  *) fxg_arch=x86_64 ;;
esac
FXG_BINARY_URL="${{FXG_BINARY_URL:-https://github.com/nazo6/flexagent/releases/download/v{version}/fxg-$fxg_os-$fxg_arch}}"
echo "[fxg-bootstrap] downloading fxg ($fxg_os-$fxg_arch) from $FXG_BINARY_URL" >&2
curl -fsSL "$FXG_BINARY_URL" -o '{binary}' >&2
chmod +x '{binary}'
{clone_step}"#,
        version = version,
        binary = EPHEMERAL_BINARY,
        clone_step = clone_step,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_script_separates_stdout_and_starts_daemon() {
        let script = compose_bootstrap_script("9.9.9", true);
        assert!(script.contains("bootstrap-workspace"));
        assert!(
            script
                .contains("exec '/tmp/fxg' daemon --stdio --ephemeral --workspace /tmp/workspace")
        );
        assert!(script.contains("v9.9.9"));
        assert!(script.contains(">&2"));
    }

    #[test]
    fn test_script_skips_workspace_clone() {
        let script = compose_bootstrap_script("9.9.9", false);
        assert!(!script.contains("bootstrap-workspace"));
        assert!(script.contains("daemon --stdio --ephemeral --workspace /tmp"));
    }

    #[test]
    fn url_host_extracts_from_https_and_scp_forms() {
        assert_eq!(
            url_host("https://github.com/nazo6/flexagent.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            url_host("https://user@gitlab.example.com:8443/x/y").as_deref(),
            Some("gitlab.example.com")
        );
        assert_eq!(
            url_host("git@github.com:nazo6/flexagent.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(url_host("github.com/nazo6/flexagent"), None);
    }
}
