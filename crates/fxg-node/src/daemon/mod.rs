//! ノードデーモン (`fxg daemon`)。
//!
//! 設計: `docs/01-architecture-and-sync.md` §1・§7、`docs/03-protocol-and-api.md` §3・§4。
//!
//! - **ローカルHTTP/WSサーバー** (`127.0.0.1:7860` 厳格バインド):
//!   クライアントAPI (REST + Client WS) を提供する ([`http`])
//! - **ローカルIPCサーバー** (Windows Named Pipe / Unix Domain Socket):
//!   `fxg` CLI からのコマンドを受け付ける ([`ipc`])
//! - **認証**: `~/.flexagent/auth_token` による Bearer / Cookie 認証 ([`auth`])

mod api;
mod auth;
mod http;
mod ipc;
mod ops;
mod stdio;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use fxg_acp::registry::AcpRegistry;
use fxg_db::{Db, DbRole};
use fxg_protocol::config::{GlobalConfig, fxg_home};
use fxg_protocol::util::now_ms;
use fxg_pty::PtySessionManager;
use fxg_server::api::ClientEvent;
use tokio::sync::{broadcast, watch};

use crate::error::NodeError;
use crate::paths::{NodePaths, ipc_endpoint};
use crate::session::SessionEventBus;
use crate::session_manager::{DriverFactory, SessionManager, default_driver_factory};

pub use auth::{generate_token, token_matches};
pub use ops::{AuditSource, WorktreeAddOutcome};
pub use stdio::{StdioConfig, run_stdio};

/// `fxg` のバージョン (このクレートの Cargo パッケージバージョン = ワークスペース版)。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Client WS 配信用イベントのブロードキャストバッファ。
const CLIENT_EVENT_CAPACITY: usize = 2048;

/// ノードデーモンの起動設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    /// ノードID (省略時は DB またはホスト名から自動解決)
    pub node_id: String,
    /// 表示名
    pub node_name: String,
    /// ローカルHTTP/WSサーバーのバインドアドレス (既定 `127.0.0.1:7860`)
    pub listen_addr: String,
    /// ローカルIPCエンドポイント (Named Pipe / UDS パス)
    pub ipc_endpoint: String,
    /// リモート (中央サーバー経由) からの Web PTY 起動を許可するか
    pub allow_remote_pty: bool,
    /// 中央サーバーの Node Hub WebSocket URL (未指定時はスタンドアロン動作)
    pub central_server_url: Option<String>,
    /// ノード個別ペアリングトークン (省略時は `~/.flexagent/node_token` を読む)
    pub node_token: Option<String>,
    /// 一時VM / サンドボックスノードか (`--ephemeral`。NodeHello と `nodes` 行に反映)
    pub ephemeral: bool,
    /// データディレクトリ (`~/.flexagent`)
    pub fxg_home: PathBuf,
}

impl DaemonConfig {
    /// 既定設定を作る (`node_id` / `node_name` は呼び出し側で解決した値)。
    pub fn new(
        fxg_home: impl Into<PathBuf>,
        node_id: impl Into<String>,
        node_name: impl Into<String>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            node_name: node_name.into(),
            listen_addr: fxg_protocol::config::NodeConfig::DEFAULT_LISTEN_ADDR.to_owned(),
            ipc_endpoint: ipc_endpoint(&fxg_protocol::config::process_env),
            allow_remote_pty: false,
            central_server_url: None,
            node_token: None,
            ephemeral: false,
            fxg_home: fxg_home.into(),
        }
    }

    /// データディレクトリと既定 IPC エンドポイントを環境から解決して構築する。
    pub fn from_env(env: fxg_protocol::config::EnvLookup<'_>) -> Self {
        let home = fxg_home(env);
        Self {
            node_id: String::new(),
            node_name: hostname(),
            listen_addr: fxg_protocol::config::NodeConfig::DEFAULT_LISTEN_ADDR.to_owned(),
            ipc_endpoint: ipc_endpoint(env),
            allow_remote_pty: false,
            central_server_url: env(fxg_protocol::config::env_keys::CENTRAL_SERVER_URL),
            node_token: env(fxg_protocol::config::env_keys::NODE_TOKEN),
            ephemeral: false,
            fxg_home: home,
        }
    }
}

/// デーモン再起動時に、前回のプロセスとともに消えたセッションの状態を整合する。
///
/// 稼働中 (`provisioning` 〜 `waiting_permission`) のまま残っているセッションは
/// エージェントプロセスが存在しない (デーモン終了で終了済み) ため、
/// `stopped` として記録し直す (`fxg ps` にゴーストセッションが残らないようにする)。
async fn reconcile_stale_sessions(bus: &SessionEventBus) -> Result<(), NodeError> {
    use fxg_db::SessionFilter;
    use fxg_protocol::common::SessionStatus;
    use fxg_protocol::events::UnifiedEventPayload;

    let filter = SessionFilter {
        statuses: DaemonState::active_statuses(),
        ..SessionFilter::default()
    };
    for session in bus.db().list_sessions(&filter).await? {
        tracing::info!(
            session_id = %session.session_id,
            "marking session as stopped (daemon restarted)"
        );
        bus.record(
            &session.session_id,
            UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Stopped,
                error_message: None,
            },
        )
        .await?;
    }
    Ok(())
}

/// ホスト名ベースのノード表示名を返す。
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "fxg-node".to_owned())
}

/// ホスト名ベースのスラッグ + 短IDでノードIDを自動採番する。
///
/// 既に `nodes` テーブルに自身の行がある場合はそれを再利用する
/// ([`DaemonState::new`] 内で解決) ため、この関数は初回起動時のみ使用される。
pub fn default_node_id() -> String {
    let host = hostname();
    let slug: String = host
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').replace("--", "-");
    let slug = if slug.is_empty() {
        "node".to_owned()
    } else {
        slug
    };
    // UUID v7 の先頭はタイムスタンプ部のため短IDには使えない。乱数から生成する。
    let short_id = format!("{:08x}", rand::random::<u32>());
    format!("{slug}-{short_id}")
}

struct Inner {
    db: Db,
    bus: SessionEventBus,
    pty: PtySessionManager,
    sessions: SessionManager,
    paths: NodePaths,
    config: DaemonConfig,
    token: RwLock<String>,
    started_at: i64,
    /// Client WS へ配信する正規化イベント (bus からの転送先)
    client_events: broadcast::Sender<ClientEvent>,
    /// 中央サーバーの Outbox 同期ワーカーが接続中か
    central_connected: AtomicBool,
    /// `NodeHello` 再送トリガ (Worktree 変更・プロジェクト登録時)
    hello_refresh: tokio::sync::Notify,
    /// Git Credential Proxy (一時VM向けオンメモリ認証中継)
    credentials: crate::credentials::CredentialBroker,
}

/// デーモンの共有状態 (HTTP ハンドラ / IPC ハンドラ / CLI から共有)。
#[derive(Clone)]
pub struct DaemonState {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DaemonState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonState")
            .field("node_id", &self.inner.config.node_id)
            .field("listen_addr", &self.inner.config.listen_addr)
            .field("ipc_endpoint", &self.inner.config.ipc_endpoint)
            .finish()
    }
}

impl DaemonState {
    /// 状態を初期化する (データディレクトリ作成 + 認証トークン読み込み +
    /// ノードID の解決)。
    pub async fn new(config: DaemonConfig) -> Result<Self, NodeError> {
        Self::with_driver_factory(config, default_driver_factory()).await
    }

    /// エージェントドライバファクトリを差し替えて状態を初期化する。
    ///
    /// 本番経路は [`Self::new`] (ACP / OpenCode2 ドライバを自動選択)。
    /// テストではモックドライバを注入して実プロセスなしで検証する。
    #[doc(hidden)]
    pub async fn with_driver_factory(
        mut config: DaemonConfig,
        driver_factory: DriverFactory,
    ) -> Result<Self, NodeError> {
        let paths = NodePaths::new(config.fxg_home.clone());
        paths.ensure_dirs()?;
        let db = Db::open(&paths.node_db_path(), DbRole::Node).await?;

        // ノードID: 設定値 → DB の既存行 → 新規自動採番 の順で解決する
        if config.node_id.trim().is_empty() {
            config.node_id = match db.list_nodes().await?.first() {
                Some(existing) => existing.node_id.clone(),
                None => default_node_id(),
            };
        }
        if config.node_name.trim().is_empty() {
            config.node_name = hostname();
        }
        config.fxg_home = paths.fxg_home().to_path_buf();

        let token = auth::load_or_create(&paths.auth_token_path())?;
        let bus = SessionEventBus::new(db.clone());
        // 前回のデーモン終了時に消えたセッションの状態を整合する
        reconcile_stale_sessions(&bus).await?;

        // ACP Registry / カスタムエージェント定義は `config.toml` から解決する
        // (読み込み失敗時は既定設定で起動し、警告のみ残す)。
        let agents = match GlobalConfig::load_from_path(&paths.config_path()) {
            Ok(global) => global.agents,
            Err(err) => {
                tracing::warn!("failed to load config.toml (using defaults): {err}");
                fxg_protocol::config::AgentsConfig::default()
            }
        };
        let registry = AcpRegistry::new(paths.fxg_home(), &agents);
        let sessions = SessionManager::with_factory(
            bus.clone(),
            paths.clone(),
            config.node_id.clone(),
            registry,
            driver_factory,
        );

        let (client_events, _) = broadcast::channel(CLIENT_EVENT_CAPACITY);
        let state = Self {
            inner: Arc::new(Inner {
                db,
                bus,
                pty: PtySessionManager::new(),
                sessions,
                paths,
                config,
                token: RwLock::new(token),
                started_at: now_ms(),
                client_events,
                central_connected: AtomicBool::new(false),
                hello_refresh: tokio::sync::Notify::new(),
                credentials: crate::credentials::CredentialBroker::new(),
            }),
        };

        // エージェントプロセスへ注入する GIT_ASKPASS ヘルパーを生成する
        // (一時VMの Git 操作はこのスクリプト → ローカルIPC → 中央サーバーの
        //  オンメモリ中継で認証する。設計: docs/01 §6.4)
        if let Err(err) = crate::credentials::write_askpass_script(state.inner.paths.fxg_home()) {
            tracing::warn!("failed to write git-askpass script: {err}");
        }

        // セッションイベントバス → Client WS 配信用イベントへ転送する
        // (bus の Lagged は Client WS ループが cursor からのリプレイで回復する)
        let mut bus_rx = state.inner.bus.subscribe();
        let client_tx = state.inner.client_events.clone();
        tokio::spawn(async move {
            loop {
                match bus_rx.recv().await {
                    Ok(crate::session::SessionBroadcast::Persisted { event, cursor }) => {
                        let _ = client_tx.send(ClientEvent::Persisted {
                            event: Box::new(event),
                            cursor,
                        });
                    }
                    Ok(crate::session::SessionBroadcast::StreamDelta { session_id, delta }) => {
                        let _ = client_tx.send(ClientEvent::StreamDelta { session_id, delta });
                    }
                    // エフェメラルイベント (キーストローク等) は PTY WS 経由で配信する
                    Ok(crate::session::SessionBroadcast::Ephemeral(_)) => {}
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "client event forwarder lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(state)
    }

    /// ローカルノードDB。
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// セッションイベントバス。
    pub fn bus(&self) -> &SessionEventBus {
        &self.inner.bus
    }

    /// ローカルPTYマネージャ。
    pub fn pty(&self) -> &PtySessionManager {
        &self.inner.pty
    }

    /// エージェントセッションマネージャ。
    pub fn session_manager(&self) -> &SessionManager {
        &self.inner.sessions
    }

    /// データディレクトリのパス解決。
    pub fn paths(&self) -> &NodePaths {
        &self.inner.paths
    }

    /// 起動設定。
    pub fn config(&self) -> &DaemonConfig {
        &self.inner.config
    }

    /// fxg のバージョン。
    pub fn version(&self) -> &'static str {
        VERSION
    }

    /// 起動からの経過ミリ秒。
    pub fn uptime_ms(&self) -> i64 {
        now_ms().saturating_sub(self.inner.started_at)
    }

    /// 中央サーバーの Outbox 同期ワーカーが接続中か。
    pub fn central_connected(&self) -> bool {
        self.inner.central_connected.load(Ordering::Relaxed)
    }

    /// 中央サーバー接続状態を更新する (Outbox 同期ワーカー専用)。
    pub fn set_central_connected(&self, connected: bool) {
        self.inner
            .central_connected
            .store(connected, Ordering::Relaxed);
    }

    /// Git Credential Proxy ブローカー。
    pub fn credentials(&self) -> &crate::credentials::CredentialBroker {
        &self.inner.credentials
    }

    /// `NodeHello` の再送を要求する (Worktree 変更等でプロジェクト紐付けを報告)。
    pub fn trigger_node_hello(&self) {
        self.inner.hello_refresh.notify_one();
    }

    /// `NodeHello` 再送トリガの通知を受け取る (同期ワーカー専用)。
    pub(crate) fn hello_refresh(&self) -> &tokio::sync::Notify {
        &self.inner.hello_refresh
    }

    /// ノードID。
    pub fn node_id(&self) -> &str {
        &self.inner.config.node_id
    }

    /// 現在のクライアント認証トークン。
    pub fn auth_token(&self) -> String {
        self.inner
            .token
            .read()
            .expect("auth token lock poisoned")
            .clone()
    }

    /// 認証トークンを再生成し、ファイル (`0600`) とメモリ上の値を更新する。
    pub fn rotate_auth_token(&self) -> Result<String, NodeError> {
        let token = auth::generate_token();
        auth::save(&self.inner.paths.auth_token_path(), &token)?;
        *self.inner.token.write().expect("auth token lock poisoned") = token.clone();
        tracing::info!("auth token rotated");
        Ok(token)
    }

    /// `nodes` テーブルへ自身を登録する (オンライン状態で upsert)。
    pub async fn register_node(&self) -> Result<(), NodeError> {
        let record = fxg_db::NodeRecord {
            node_id: self.inner.config.node_id.clone(),
            name: self.inner.config.node_name.clone(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            version: VERSION.to_owned(),
            installed_agents: self.installed_agents().await,
            is_ephemeral: self.inner.config.ephemeral,
            provisioner: None,
            lifecycle_status: fxg_protocol::common::NodeLifecycleStatus::Ready,
            idle_timeout_secs: None,
            is_online: true,
        };
        self.inner.db.upsert_node(&record).await?;
        Ok(())
    }

    /// 利用可能なエージェントID一覧 (ACP Registry 導入済み + カスタム/ビルトイン)。
    ///
    /// レジストリの取得・展開に時間がかかる環境でもデーモン起動をブロックしない
    /// よう、短いタイムアウト付きで解決する (失敗時は空一覧)。
    pub async fn installed_agents(&self) -> Vec<String> {
        let registry = self.inner.sessions.registry();
        let listed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let index = registry.index(false).await?;
            Ok::<_, anyhow::Error>(
                registry
                    .list(&index, false)
                    .into_iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>(),
            )
        })
        .await;
        match listed {
            Ok(Ok(agents)) => agents,
            Ok(Err(err)) => {
                tracing::debug!("failed to list installed agents: {err}");
                Vec::new()
            }
            Err(_) => {
                tracing::debug!("listing installed agents timed out");
                Vec::new()
            }
        }
    }

    /// ノードをオフラインとして記録する (終了時)。
    pub async fn mark_offline(&self) -> Result<(), NodeError> {
        self.inner
            .db
            .set_node_online(&self.inner.config.node_id, false)
            .await?;
        Ok(())
    }

    /// ローカルで稼働中とみなすセッションの状態一覧 (`fxg ps` の既定フィルタ)。
    pub fn active_statuses() -> Vec<fxg_protocol::common::SessionStatus> {
        use fxg_protocol::common::SessionStatus;
        vec![
            SessionStatus::Provisioning,
            SessionStatus::Bootstrapping,
            SessionStatus::Idle,
            SessionStatus::Running,
            SessionStatus::WaitingPermission,
        ]
    }

    /// ローカルノード上の全セッション・PTYを強制終了する (緊急キルスイッチ)。
    ///
    /// - PTY: [`PtySessionManager::kill_all`] (Windows では Job Object 経由で
    ///   プロセスツリーごと終了)
    /// - エージェントセッション: [`SessionManager::shutdown_all`]
    ///   (ドライバ経由でエージェントプロセスツリーごと終了)
    /// - 実行中だったセッション: `StatusChanged(Stopped)` イベントを発行
    ///
    /// 監査ログ (`audit_logs`) の記録はクライアント情報 (IP / 経路) を持つ
    /// 呼び出し側 (IPC / HTTP ハンドラ) の責務とする。
    ///
    /// 終了させたセッションIDと PTY IDの一覧を返す。
    pub async fn kill_all_local(
        &self,
        reason: &str,
    ) -> Result<(Vec<String>, Vec<String>), NodeError> {
        use fxg_protocol::common::SessionStatus;
        use fxg_protocol::events::UnifiedEventPayload;

        let killed_ptys = self.inner.pty.kill_all();

        // エージェントプロセスを停止する (プロセスツリーごと)。
        let stopped_agents = self.inner.sessions.shutdown_all().await;
        if !stopped_agents.is_empty() {
            tracing::info!(count = stopped_agents.len(), "stopped local agent sessions");
        }

        let sessions = self
            .inner
            .db
            .list_sessions(&fxg_db::SessionFilter {
                statuses: Self::active_statuses(),
                ..fxg_db::SessionFilter::default()
            })
            .await?;

        let mut killed_sessions = Vec::new();
        for session in sessions {
            match self
                .inner
                .bus
                .record(
                    &session.session_id,
                    UnifiedEventPayload::StatusChanged {
                        status: SessionStatus::Stopped,
                        error_message: Some(reason.to_owned()),
                    },
                )
                .await
            {
                Ok(_) => killed_sessions.push(session.session_id),
                Err(err) => {
                    tracing::warn!("failed to stop session {}: {err}", session.session_id);
                }
            }
        }

        Ok((killed_sessions, killed_ptys))
    }
}

/// Outbox Sync Worker を起動する (中央サーバー URL 未設定時は `None`)。
///
/// ノード個別トークンは `config.node_token` / `FXG_NODE_TOKEN` →
/// `~/.flexagent/node_token` の順で解決する。
fn start_sync_worker(
    state: &DaemonState,
    shutdown_tx: &watch::Sender<bool>,
) -> Result<Option<tokio::task::JoinHandle<()>>, NodeError> {
    let Some(url) = state.config().central_server_url.clone() else {
        return Ok(None);
    };
    let token = match state.config().node_token.clone() {
        Some(token) if !token.trim().is_empty() => token,
        _ => match std::fs::read_to_string(state.paths().node_token_path()) {
            Ok(token) if !token.trim().is_empty() => token.trim().to_owned(),
            _ => {
                tracing::warn!(
                    "central_server_url is set but node_token is missing; outbox sync disabled"
                );
                return Ok(None);
            }
        },
    };
    let state = state.clone();
    let shutdown = shutdown_tx.subscribe();
    let task = tokio::spawn(async move {
        crate::sync::run(state, url, token, shutdown).await;
    });
    Ok(Some(task))
}

/// 起動済みのノードデーモン。
pub struct NodeDaemon {
    state: DaemonState,
    http_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    http_task: tokio::task::JoinHandle<()>,
    ipc_task: tokio::task::JoinHandle<()>,
    sync_task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for NodeDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeDaemon")
            .field("node_id", &self.state.node_id())
            .field("http_addr", &self.http_addr)
            .field("ipc_endpoint", &self.state.config().ipc_endpoint)
            .finish()
    }
}

impl NodeDaemon {
    /// デーモンを起動する (データディレクトリ作成・ノード登録・
    /// ローカルHTTP/WS と ローカルIPC のリッスン開始)。
    pub async fn start(config: DaemonConfig) -> Result<Self, NodeError> {
        Self::start_with_driver_factory(config, default_driver_factory()).await
    }

    /// エージェントドライバファクトリを差し替えてデーモンを起動する
    /// (テスト用。本番経路は [`Self::start`])。
    #[doc(hidden)]
    pub async fn start_with_driver_factory(
        config: DaemonConfig,
        driver_factory: DriverFactory,
    ) -> Result<Self, NodeError> {
        let state = DaemonState::with_driver_factory(config, driver_factory).await?;
        state.register_node().await?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // ローカルHTTP/WS サーバー (ループバック厳格バインド)
        let listen_addr = state.config().listen_addr.clone();
        let listener = tokio::net::TcpListener::bind(&listen_addr)
            .await
            .map_err(|err| NodeError::Server(format!("failed to bind {listen_addr}: {err}")))?;
        let http_addr = listener
            .local_addr()
            .map_err(|err| NodeError::Server(err.to_string()))?;

        let http_state = state.clone();
        let http_shutdown = shutdown_rx.clone();
        let http_task = tokio::spawn(async move {
            http::serve(http_state, listener, http_shutdown).await;
        });

        // ローカルIPCサーバー (Named Pipe / UDS)。
        // 別デーモンが同じエンドポイントで稼働していないかを先に確認する
        // (障害を `start` の呼び出し元へ返すため)。
        ipc::check_endpoint_available(&state.config().ipc_endpoint).await?;
        let ipc_state = state.clone();
        let ipc_endpoint = state.config().ipc_endpoint.clone();
        let ipc_task = tokio::spawn(async move {
            if let Err(err) = ipc::serve(ipc_state, ipc_endpoint, shutdown_rx).await {
                tracing::error!("local ipc server stopped: {err}");
            }
        });

        // Ctrl-C でシャットダウンを要求する
        let signal_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("received Ctrl-C, shutting down");
                let _ = signal_tx.send(true);
            }
        });

        // Outbox Sync Worker (中央サーバー URL 設定時のみ起動)
        let sync_task = start_sync_worker(&state, &shutdown_tx)?;

        tracing::info!(
            node_id = state.config().node_id,
            http = %http_addr,
            ipc = %state.config().ipc_endpoint,
            "fxg daemon started"
        );

        Ok(Self {
            state,
            http_addr,
            shutdown_tx,
            http_task,
            ipc_task,
            sync_task,
        })
    }

    /// 共有状態。
    pub fn state(&self) -> &DaemonState {
        &self.state
    }

    /// ローカルHTTP/WSサーバーの実バインドアドレス。
    pub fn http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    /// ローカルIPCエンドポイント。
    pub fn ipc_endpoint(&self) -> &str {
        &self.state.config().ipc_endpoint
    }

    /// シャットダウンを要求する (実際の停止は [`Self::wait`] を待つ)。
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// シャットダウン要求まで待機し、後処理 (PTY 停止・オフライン記録) を行って終了する。
    pub async fn wait(self) {
        let mut receiver = self.shutdown_tx.subscribe();
        // `watch` の受信者は生成時点の値を「既読」として開始するため、
        // 先に `shutdown()` 済みかどうかを現在値で確認してから待機する。
        if !*receiver.borrow_and_update() {
            let _ = receiver.changed().await;
        }
        let _ = self.shutdown_tx.send(true);

        let stopped = self.state.session_manager().shutdown_all().await;
        if !stopped.is_empty() {
            tracing::info!(count = stopped.len(), "stopped agent sessions on shutdown");
        }
        let killed = self.state.pty().kill_all();
        if !killed.is_empty() {
            tracing::info!(count = killed.len(), "killed local ptys on shutdown");
        }
        if let Err(err) = self.state.mark_offline().await {
            tracing::warn!("failed to mark node offline: {err}");
        }

        let _ = self.http_task.await;
        let _ = self.ipc_task.await;
        if let Some(sync_task) = self.sync_task {
            let _ = sync_task.await;
        }
        tracing::info!("fxg daemon stopped");
    }

    /// デーモンを起動し、シャットダウンまで稼働する (CLI エントリポイント用)。
    pub async fn run(config: DaemonConfig) -> Result<(), NodeError> {
        let daemon = Self::start(config).await?;
        daemon.wait().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

    fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn default_node_id_is_slugged_and_unique() {
        let a = default_node_id();
        let b = default_node_id();
        assert_ne!(a, b, "短IDは毎回異なる");
        assert!(!a.contains(' '));
        assert!(
            a.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        );
    }

    #[test]
    fn daemon_config_from_env_uses_fxg_home() {
        let env = env_map(&[("FXG_HOME", "/tmp/fxg-test-home")]);
        let config = DaemonConfig::from_env(&|key| env.get(key).cloned());
        assert_eq!(config.fxg_home, PathBuf::from("/tmp/fxg-test-home"));
        assert_eq!(
            config.listen_addr,
            fxg_protocol::config::NodeConfig::DEFAULT_LISTEN_ADDR
        );
        assert!(!config.allow_remote_pty);
        assert!(!config.ipc_endpoint.is_empty());
    }

    async fn start_test_daemon() -> (NodeDaemon, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "test-node", "Test Node");
        // テストではポート競合を避けるため ephemeral port と一時IPCパスを使う
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let daemon = NodeDaemon::start(config).await.expect("start daemon");
        (daemon, dir)
    }

    #[tokio::test]
    async fn daemon_starts_and_registers_itself() {
        let (daemon, _dir) = start_test_daemon().await;
        assert_ne!(daemon.http_addr().port(), 0, "ephemeral port is bound");

        let nodes = daemon.state().db().list_nodes().await.expect("nodes");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "test-node");
        assert!(nodes[0].is_online);
        assert_eq!(nodes[0].os, std::env::consts::OS);

        // 認証トークンが生成されている
        let token = daemon.state().auth_token();
        assert_eq!(token.len(), 64);
        assert!(daemon.state().paths().auth_token_path().is_file());

        daemon.shutdown();
        tokio::time::timeout(Duration::from_secs(5), daemon.wait())
            .await
            .expect("daemon stops within timeout");
    }

    #[tokio::test]
    async fn reconcile_marks_stale_sessions_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory(DbRole::Node).await.expect("db");
        db.upsert_node(&fxg_db::NodeRecord::new(
            "test-node",
            "Test Node",
            "windows",
            "x86_64",
            "0.1.0",
        ))
        .await
        .expect("node");
        let bus = SessionEventBus::new(db.clone());
        // 前回のデーモンが強制終了され、`idle` のまま残ったセッションを再現する
        bus.create_session(
            "stale-session",
            fxg_protocol::events::UnifiedEventPayload::SessionCreated {
                node_id: "test-node".to_owned(),
                project_id: "github.com/nazo6/flexagent".to_owned(),
                project_name: "flexagent".to_owned(),
                local_path: dir.path().to_string_lossy().into_owned(),
                git_branch: None,
                is_worktree: false,
                agent_id: "mock".to_owned(),
                parent_session_id: None,
                fork_from_node_seq: None,
                title: "stale".to_owned(),
            },
        )
        .await
        .expect("create session");

        reconcile_stale_sessions(&bus).await.expect("reconcile");

        let session = db
            .get_session("stale-session")
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(session.status, fxg_protocol::common::SessionStatus::Stopped);
    }

    #[tokio::test]
    async fn daemon_reuses_node_id_from_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "persisted-node", "Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let daemon = NodeDaemon::start(config).await.expect("start");
        daemon.shutdown();
        daemon.wait().await;

        // node_id 未指定で再起動しても同じ ID を使う
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "", "");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let daemon = NodeDaemon::start(config).await.expect("restart");
        assert_eq!(daemon.state().node_id(), "persisted-node");
        assert_eq!(daemon.state().config().node_name, hostname());
        daemon.shutdown();
        daemon.wait().await;
    }

    #[tokio::test]
    async fn rotate_auth_token_persists_new_value() {
        let (daemon, _dir) = start_test_daemon().await;
        let before = daemon.state().auth_token();
        let after = daemon.state().rotate_auth_token().expect("rotate");
        assert_ne!(before, after);
        assert_eq!(daemon.state().auth_token(), after);

        let on_disk = std::fs::read_to_string(daemon.state().paths().auth_token_path())
            .expect("read token file");
        assert_eq!(on_disk.trim(), after);

        daemon.shutdown();
        daemon.wait().await;
    }

    #[tokio::test]
    async fn kill_all_stops_agent_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::testutil::write_mock_agent_config(dir.path());
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "kill-node", "Kill Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let mock = crate::testutil::MockAgent::default();
        let daemon = NodeDaemon::start_with_driver_factory(config, mock.factory())
            .await
            .expect("start");

        let workdir = dir.path().join("workspace");
        std::fs::create_dir_all(&workdir).expect("mkdir");
        let outcome = daemon
            .state()
            .session_manager()
            .ensure_session("c1", &workdir, "mock", &[], None, false)
            .await
            .expect("ensure session");
        assert_eq!(daemon.state().session_manager().list_active().len(), 1);

        // キルスイッチでエージェントセッションが停止し、Stopped が記録される
        let (killed_sessions, killed_ptys) = daemon
            .state()
            .kill_all_local("kill switch test")
            .await
            .expect("kill all");
        assert_eq!(killed_sessions, vec![outcome.session_id.clone()]);
        assert!(killed_ptys.is_empty());
        assert!(daemon.state().session_manager().list_active().is_empty());

        let sessions = daemon
            .state()
            .db()
            .list_sessions(&fxg_db::SessionFilter::default())
            .await
            .expect("list sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].status,
            fxg_protocol::common::SessionStatus::Stopped
        );

        daemon.shutdown();
        daemon.wait().await;
    }
}
