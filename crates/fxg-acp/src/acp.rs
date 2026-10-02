//! 標準ACPエージェントのドライバ (`AcpDriver`)。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §2.2。
//!
//! `agent-client-protocol` (ACP SDK) のクライアントとして外部エージェント
//! プロセスへ接続し、`session/update` 通知・`session/request_permission`・
//! `fs/*`・`terminal/*` を FXG の正規化イベント ([`DriverEvent`]) へ変換する。
//!
//! - **セッション所有**: ACP接続は専用タスクが所有し、
//!   [`ActiveSession`] の更新読み取りとプロンプト送信を直列化する。
//!   外部からの操作は [`ConnectionTo`] (clone 可能) とコマンドチャネル経由。
//! - **承認**: `request_permission` は `oneshot` を登録して
//!   [`ActiveSessionHandle::respond_permission`] から解決する
//!   (未解決のままターンがキャンセルされた場合は `cancelled` を返す)。
//! - **fs/terminal**: セッションの `local_path` 基準で読み書きし、
//!   書き込みは Unified Diff を計算してイベントへ添付する。
//!   `terminal/*` は `fxg-pty` で実行する。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ClientCapabilities, CompleteElicitationNotification,
    ContentBlock, ContentChunk, CreateElicitationRequest, CreateElicitationResponse,
    CreateTerminalRequest, CreateTerminalResponse, ElicitationAcceptAction,
    ElicitationAction as AcpElicitationAction, ElicitationCapabilities, ElicitationContentValue,
    ElicitationFormCapabilities, ElicitationMode, ElicitationScope, FileSystemCapabilities,
    Implementation, InitializeRequest, KillTerminalRequest, KillTerminalResponse,
    NewSessionRequest, PermissionOptionKind, ReadTextFileRequest, ReadTextFileResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionConfigKind, SessionConfigOption, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, TerminalExitStatus, TerminalId,
    TerminalOutputRequest, TerminalOutputResponse, TextContent, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolKind, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, ActiveSession, Agent, ByteStreams, Client, ConnectionTo,
    SessionMessage,
};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use fxg_protocol::common::{
    CommandInfo, ConfigOptionInfo, FileDiff, ModeInfo, PermissionOption, PlanEntry,
    StreamDeltaPayload,
};
use fxg_protocol::events::UnifiedEventPayload;
use fxg_protocol::util::uuid_v7;
use tokio::sync::{mpsc, oneshot, watch};

use crate::driver::{
    ActiveSessionHandle, AgentDriver, DriverEvent, NativeResumeUnavailable, ResumeRequest,
    StartSessionRequest, StartedSession,
};
use crate::warm::{DEFAULT_WARM_IDLE, LaunchKey, WarmPool, WarmProcess};

/// 標準ACPエージェントを `agent-client-protocol` で制御するドライバ。
///
/// セッション終了後もプロセスを TTL 付きでアイドル保持し、次のセッションで
/// 再利用する (起動が高コストなエージェントでは 2 セッション目以降の開始が
/// `session/new` のみで済む)。プール無効化は [`AcpDriver::without_pool`]。
pub struct AcpDriver {
    /// アイドルプロセスの再利用プール (`None` = プール無効)
    pool: Option<Arc<WarmPool<ProcessHandle>>>,
}

impl std::fmt::Debug for AcpDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpDriver")
            .field("warm_pool", &self.pool.is_some())
            .finish()
    }
}

impl Default for AcpDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpDriver {
    /// アイドルプール有効 (既定 TTL 10 分) でドライバを作成する。
    pub fn new() -> Self {
        Self {
            pool: Some(Arc::new(WarmPool::new(DEFAULT_WARM_IDLE))),
        }
    }

    /// アイドルプール無効でドライバを作成する (セッション終了でプロセスを破棄)。
    ///
    /// セッション間でエージェント状態を共有したくない場合 (テスト・診断) に使う。
    pub fn without_pool() -> Self {
        Self { pool: None }
    }
}

/// 承認リクエストの解決チャネル (`request_id` → `oneshot`)。
type PermissionRegistry = Arc<Mutex<HashMap<String, oneshot::Sender<RequestPermissionOutcome>>>>;

/// elicitation の解決チャネル (`elicitation_id` → 未解決情報)。
type ElicitationRegistry = Arc<Mutex<HashMap<String, PendingElicitation>>>;

/// 未解決 elicitation の状態 (応答チャネルと検証用 schema)。
struct PendingElicitation {
    /// UI / CLI からの応答送信側
    sender: oneshot::Sender<ElicitationOutcome>,
    /// form モードの要求 schema (`accept` の content 検証用)
    schema: Option<serde_json::Value>,
}

/// elicitation への応答 (ACP 応答へ変換する前の正規化表現)。
enum ElicitationOutcome {
    /// accept (form content 付き)
    Accept(BTreeMap<String, ElicitationContentValue>),
    /// decline
    Decline,
    /// cancel
    Cancel,
}

/// エージェントのブートストラップ (ラッパープロセス) の自然終了を待つ猶予。
///
/// PyInstaller onefile 型のエージェントは子 (実体) の終了後に展開済み一時
/// ファイル (数百 MB) を削除してから終了するため、削除 I/O を見込んだ余裕を取る。
const AGENT_EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(15);

/// セッション (ターンループ) へのコマンド。
#[derive(Debug)]
enum AcpCommand {
    /// プロンプト送信 (ターン完了まで実行される)
    Prompt(String),
    /// セッション終了 (プロセスは維持し、次のセッションで再利用する)
    Shutdown,
}

/// 常駐プロセスへのコマンド。
enum ProcessCommand {
    /// 新規セッションを作成してターンループを開始する
    NewSession {
        /// セッション作成パラメータ
        params: SessionParams,
        /// セッションのイベント送信
        events: mpsc::UnboundedSender<DriverEvent>,
        /// セッションが握るプロセスの利用権
        lease: Arc<ProcessLease>,
        /// セッションへのコマンドチャネル (送信側はハンドルが保持)
        commands: (
            mpsc::UnboundedSender<AcpCommand>,
            mpsc::UnboundedReceiver<AcpCommand>,
        ),
        /// セッション準備完了通知 (ハンドル + ネイティブ復元成否 / エラー)
        ready: oneshot::Sender<anyhow::Result<(AcpSessionHandle, bool)>>,
    },
    /// プロセスを終了する (teardown 実行)
    Dispose,
}

/// プロセス内で新規セッションを作るために必要なパラメータ。
#[derive(Debug, Clone)]
struct SessionParams {
    /// fxg 側のセッションID (ログ用)
    session_id: String,
    /// 作業ディレクトリ (セッションの `local_path`)
    cwd: PathBuf,
    /// 初期モード
    initial_mode: Option<String>,
    /// 既存エージェントセッションからの再開指定 (`None` は新規セッション)
    resume: Option<ResumeRequest>,
}

/// セッション単位の共有情報 (ハンドラが ACP の session_id から引く)。
///
/// 1 プロセスがセッションをまたいで再利用されるため、fs / terminal
/// ハンドラは接続構築時の情報を閉じ込めず、ここから都度解決する。
#[derive(Clone)]
struct SessionContext {
    /// 作業ディレクトリ (fs / terminal の相対パス解決に使う)
    cwd: PathBuf,
    /// イベント送信 (ステータス更新・承認・fs 変更通知)
    events: mpsc::UnboundedSender<DriverEvent>,
}

/// ACP セッションID → セッション情報。
type SessionContexts = Arc<Mutex<HashMap<String, SessionContext>>>;

/// ACP セッションID でセッション情報を引く。
fn session_context(contexts: &SessionContexts, session_id: &SessionId) -> Option<SessionContext> {
    contexts
        .lock()
        .expect("session contexts poisoned")
        .get(&session_id.to_string())
        .cloned()
}

/// 常駐プロセスのハンドル (プール / リースが共有保持する)。
struct ProcessHandle {
    /// プロセスへのコマンド送信
    control_tx: mpsc::UnboundedSender<ProcessCommand>,
    /// 常駐タスクの JoinHandle (`dispose_process` が teardown 完了を待つ)
    join: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ProcessHandle {
    /// 新しいセッションを作成し、ハンドルとネイティブ復元成否を返す。
    async fn new_session(
        &self,
        params: SessionParams,
        events: mpsc::UnboundedSender<DriverEvent>,
        lease: Arc<ProcessLease>,
    ) -> anyhow::Result<(AcpSessionHandle, bool)> {
        let commands = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        self.control_tx
            .send(ProcessCommand::NewSession {
                params,
                events,
                lease,
                commands,
                ready: ready_tx,
            })
            .map_err(|_| anyhow!("acp agent process is no longer running"))?;
        ready_rx
            .await
            .context("acp agent process ended before the session became ready")?
    }

    /// プロセスを終了し、teardown の完了を待つ (完了済みなら即時)。
    async fn dispose_process(&self) {
        let _ = self.control_tx.send(ProcessCommand::Dispose);
        let join = self.join.lock().expect("process join poisoned").take();
        if let Some(join) = join {
            let _ = join.await;
        }
    }
}

impl WarmProcess for ProcessHandle {
    fn dispose(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.dispose_process())
    }
}

/// セッションが握るプロセスの利用権。終了時にプールへ返却、または破棄する。
struct ProcessLease {
    /// 再利用キー
    key: LaunchKey,
    /// 常駐プロセス
    process: Arc<ProcessHandle>,
    /// 返却先プール (`None` = プール無効 → 破棄)
    pool: Option<Arc<WarmPool<ProcessHandle>>>,
    /// [`AcpSessionHandle::dispose`] 指定 (プールへ返さずプロセスを破棄する)
    dispose_requested: AtomicBool,
    /// 返却 / 破棄の二重実行防止
    finished: AtomicBool,
    /// ターンループ終了通知の送信側 (プロセス側が serve 開始時に取り出す)
    session_end_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// ターンループ終了通知の受信側 (`shutdown` が待つ)
    session_end_rx: Mutex<Option<oneshot::Receiver<()>>>,
}

impl std::fmt::Debug for ProcessLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessLease")
            .field("key", &self.key)
            .field("dispose_requested", &self.dispose_requested)
            .field("finished", &self.finished)
            .finish()
    }
}

impl ProcessLease {
    /// 新しい利用権を作成する (ターンループ終了通知チャネルを含む)。
    fn new(
        key: LaunchKey,
        process: Arc<ProcessHandle>,
        pool: Option<Arc<WarmPool<ProcessHandle>>>,
    ) -> Self {
        let (session_end_tx, session_end_rx) = oneshot::channel();
        Self {
            key,
            process,
            pool,
            dispose_requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            session_end_tx: Mutex::new(Some(session_end_tx)),
            session_end_rx: Mutex::new(Some(session_end_rx)),
        }
    }

    /// ターンループ終了通知の送信側を取り出す (プロセス側が一度だけ)。
    fn take_session_end_sender(&self) -> Option<oneshot::Sender<()>> {
        self.session_end_tx
            .lock()
            .expect("process lease poisoned")
            .take()
    }

    /// ターンループ終了を待つための受信側を取り出す (`shutdown` が一度だけ)。
    fn take_session_end_receiver(&self) -> Option<oneshot::Receiver<()>> {
        self.session_end_rx
            .lock()
            .expect("process lease poisoned")
            .take()
    }

    /// プールへ返さずプロセスを破棄するよう印を付ける (セッション異常終了時など)。
    fn mark_dispose(&self) {
        self.dispose_requested.store(true, Ordering::SeqCst);
    }

    /// セッション終了後の後処理 (プールへ返却 or プロセス破棄)。二重呼び出しは無視する。
    async fn finish(&self) {
        if self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.dispose_requested.load(Ordering::SeqCst) || self.pool.is_none() {
            self.process.dispose_process().await;
        } else if let Some(pool) = &self.pool {
            pool.release(self.key.clone(), Arc::clone(&self.process));
        }
    }
}

#[async_trait]
impl AgentDriver for AcpDriver {
    fn driver_kind(&self) -> &'static str {
        "acp"
    }

    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        match self.start_session_inner(req, &event_tx).await {
            Ok(started) => Ok(started),
            Err(err) => {
                // 起動失敗をイベントに記録する (セッションは error 状態へ遷移する)
                let _ = event_tx.send(DriverEvent::Failed {
                    message: err.to_string(),
                });
                Err(err)
            }
        }
    }

    async fn shutdown_idle(&self) -> usize {
        match &self.pool {
            Some(pool) => pool.shutdown_idle().await,
            None => 0,
        }
    }
}

impl AcpDriver {
    /// セッション開始の本体 (アイドルプロセスの再利用 → コールドスタートの順)。
    async fn start_session_inner(
        &self,
        req: StartSessionRequest,
        event_tx: &mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        // `npx` / `uvx` 等の拡張子解決 (Windows の PATHEXT 対応)
        let program = fxg_pty::resolve_command(&req.launch.program.to_string_lossy(), &req.cwd)
            .map_err(|err| {
                anyhow!(
                    "failed to resolve agent program {}: {err}",
                    req.launch.program.display()
                )
            })?;
        let key = LaunchKey::new(&req.launch, program.clone());
        let session_params = SessionParams {
            session_id: req.session_id.clone(),
            cwd: req.cwd.clone(),
            initial_mode: req.initial_mode.clone(),
            resume: req.resume.clone(),
        };

        // 1) アイドルプロセスの再利用 (initialize 済みの接続へ session/new するだけ)
        if let Some(pool) = &self.pool
            && let Some(process) = pool.checkout(&key)
        {
            let lease = Arc::new(ProcessLease::new(
                key.clone(),
                Arc::clone(&process),
                Some(Arc::clone(pool)),
            ));
            match process
                .new_session(session_params.clone(), event_tx.clone(), lease)
                .await
            {
                Ok((handle, context_restored)) => {
                    tracing::debug!(
                        session_id = %req.session_id,
                        agent = %key.agent_id(),
                        "reusing idle acp agent process"
                    );
                    return Ok(StartedSession {
                        handle: Box::new(handle),
                        context_restored,
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        session_id = %req.session_id,
                        agent = %key.agent_id(),
                        "failed to reuse idle acp agent process: {err:#}"
                    );
                    process.dispose_process().await;
                }
            }
        }

        // 2) コールドスタート (プロセス起動 → initialize → セッション作成)
        let config = AcpAgentConfig::new(program)
            .args(req.launch.args.iter().cloned())
            .envs(
                req.launch
                    .env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        let (process, ready_rx) = spawn_agent_process(AcpAgent::new(config));
        ready_rx
            .await
            .context("acp agent process ended during startup")??;
        let lease = Arc::new(ProcessLease::new(
            key,
            Arc::clone(&process),
            self.pool.clone(),
        ));
        let (handle, context_restored) = process
            .new_session(session_params, event_tx.clone(), lease)
            .await?;
        Ok(StartedSession {
            handle: Box::new(handle),
            context_restored,
        })
    }
}

/// エージェントプロセスを起動し、常駐タスクとプロセス ready 通知を返す。
///
/// ready は `initialize` 完了時に通知される (セッション作成は
/// [`ProcessHandle::new_session`] で行う)。
fn spawn_agent_process(
    agent: AcpAgent,
) -> (Arc<ProcessHandle>, oneshot::Receiver<anyhow::Result<()>>) {
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        if let Err(err) = run_acp_process(AcpProcessTaskParams {
            agent,
            ready_tx,
            control_rx,
        })
        .await
        {
            tracing::warn!("acp agent process ended: {err:#}");
        }
    });
    (
        Arc::new(ProcessHandle {
            control_tx,
            join: Mutex::new(Some(join)),
        }),
        ready_rx,
    )
}

/// 起動済みACPセッションのハンドル。
struct AcpSessionHandle {
    /// ACP (エージェント側) のセッションID
    acp_session_id: SessionId,
    /// 接続 (clone 可能。リクエスト/通知の発行に使う)
    conn: ConnectionTo<Agent>,
    /// セッション (ターンループ) へのコマンド
    commands: mpsc::UnboundedSender<AcpCommand>,
    /// 承認解決レジストリ (プロセス共有)
    permissions: PermissionRegistry,
    /// elicitation 解決レジストリ (プロセス共有)
    elicitations: ElicitationRegistry,
    /// イベント送信 (ステータス更新用)。
    ///
    /// セッション終了 ([`ActiveSessionHandle::shutdown`]) で解放する。保持した
    /// ままだとセッションマネージャのイベントポンプが「ドライバ終了」を検知
    /// できず、セッションが active 一覧から外れない。
    events: Mutex<Option<mpsc::UnboundedSender<DriverEvent>>>,
    /// PTY マネージャ (ACP terminal/* 用。プロセス共有)
    pty: Arc<fxg_pty::PtySessionManager>,
    /// プロセスの利用権 (終了時にプールへ返却 or 破棄)
    lease: Arc<ProcessLease>,
}

#[async_trait]
impl ActiveSessionHandle for AcpSessionHandle {
    async fn send_prompt(&self, text: String) -> anyhow::Result<()> {
        self.commands
            .send(AcpCommand::Prompt(text))
            .map_err(|_| anyhow!("acp session is no longer running"))
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        let sender = self
            .permissions
            .lock()
            .expect("permission registry poisoned")
            .remove(&request_id);
        let Some(sender) = sender else {
            return Err(anyhow!(
                "unknown or already resolved permission: {request_id}"
            ));
        };
        let outcome = if selected_option_id.is_empty() {
            RequestPermissionOutcome::Cancelled
        } else {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(selected_option_id))
        };
        sender
            .send(outcome)
            .map_err(|_| anyhow!("permission resolver dropped: {request_id}"))
    }

    async fn respond_elicitation(
        &self,
        elicitation_id: String,
        action: fxg_protocol::common::ElicitationAction,
        content: serde_json::Value,
    ) -> anyhow::Result<()> {
        use fxg_protocol::common::ElicitationAction as FxgElicitationAction;

        // schema は pending を消費せずに読む (不正な content は pending を維持して
        // エラーを返し、クライアントが修正して再送できるようにする)
        let schema = self
            .elicitations
            .lock()
            .expect("elicitation registry poisoned")
            .get(&elicitation_id)
            .map(|pending| pending.schema.clone())
            .ok_or_else(|| anyhow!("unknown or already resolved elicitation: {elicitation_id}"))?;
        let outcome = match action {
            FxgElicitationAction::Accept => {
                // `content` 未指定 (null) は空オブジェクトとして扱う
                let content = if content.is_null() {
                    serde_json::json!({})
                } else {
                    content
                };
                if let Some(schema) = schema.as_ref() {
                    validate_elicitation_content(schema, &content)
                        .map_err(|err| anyhow!("invalid elicitation content: {err}"))?;
                }
                let content =
                    serde_json::from_value::<BTreeMap<String, ElicitationContentValue>>(content)
                        .map_err(|err| anyhow!("unsupported elicitation content value: {err}"))?;
                ElicitationOutcome::Accept(content)
            }
            FxgElicitationAction::Decline => ElicitationOutcome::Decline,
            FxgElicitationAction::Cancel => ElicitationOutcome::Cancel,
        };
        let sender = self
            .elicitations
            .lock()
            .expect("elicitation registry poisoned")
            .remove(&elicitation_id)
            .map(|pending| pending.sender)
            .ok_or_else(|| anyhow!("unknown or already resolved elicitation: {elicitation_id}"))?;
        sender
            .send(outcome)
            .map_err(|_| anyhow!("elicitation resolver dropped: {elicitation_id}"))
    }

    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()> {
        self.conn
            .send_request(SetSessionModeRequest::new(
                self.acp_session_id.clone(),
                mode_id,
            ))
            .block_task()
            .await
            .map_err(|err| anyhow!("failed to set mode: {err}"))?;
        Ok(())
    }

    async fn set_config(&self, key: String, value: serde_json::Value) -> anyhow::Result<()> {
        let value: agent_client_protocol::schema::v1::SessionConfigOptionValue =
            serde_json::from_value(value)
                .map_err(|err| anyhow!("unsupported config value for {key}: {err}"))?;
        self.conn
            .send_request(SetSessionConfigOptionRequest::new(
                self.acp_session_id.clone(),
                key,
                value,
            ))
            .block_task()
            .await
            .map_err(|err| anyhow!("failed to set config option: {err}"))?;
        Ok(())
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        self.conn
            .send_notification(CancelNotification::new(self.acp_session_id.clone()))
            .map_err(|err| anyhow!("failed to cancel turn: {err}"))?;
        // 未解決の承認は仕様上 cancelled で返す (MUST)
        cancel_pending_permissions(&self.permissions);
        // 未解決の elicitation も仕様上 cancel で返し、回答待ち表示を解消する
        let events = self.events.lock().expect("session events poisoned").clone();
        cancel_pending_elicitations(&self.elicitations, events.as_ref());
        if let Some(events) = events.as_ref() {
            let _ = events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: fxg_protocol::common::SessionStatus::Running,
                error_message: None,
            }));
        }
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        // セッションを閉じ、プロセスは再利用プールへ返す (破棄は [`Self::dispose`])
        let session_end = self.lease.take_session_end_receiver();
        let _ = self.commands.send(AcpCommand::Shutdown);
        if let Some(session_end) = session_end {
            // ターンループの終了を待つ (ループ未起動/異常終了時は即時 Err で返る)
            let _ = session_end.await;
        }
        for terminal_id in self.pty.list() {
            let _ = self.pty.kill(&terminal_id.pty_id);
        }
        // イベント送信を解放する (セッションマネージャのポンプが「ドライバ終了」
        // を検知し、active 一覧から外せるようにするため)
        self.events.lock().expect("session events poisoned").take();
        self.lease.finish().await;
        Ok(())
    }

    async fn dispose(&self) -> anyhow::Result<()> {
        // プロセスを温存せず破棄する (`fxg kill-all` / デーモン終了)
        self.lease.dispose_requested.store(true, Ordering::SeqCst);
        self.shutdown().await
    }
}

/// 未解決の承認リクエストをすべて `cancelled` で閉じる。
fn cancel_pending_permissions(permissions: &PermissionRegistry) {
    let mut registry = permissions.lock().expect("permission registry poisoned");
    for (_, sender) in registry.drain() {
        let _ = sender.send(RequestPermissionOutcome::Cancelled);
    }
}

/// 未解決の elicitation をすべて `cancel` で閉じる (仕様上、ターン中断時は
/// 未解決の elicitation を cancel で解決する)。
///
/// `events` がある場合は `ElicitationResolved` も発行し、UI / Inbox の
/// 回答待ち表示を解消する (fxg 経由の応答はマネージャ側が記録済みのため、
/// ここではドライバ側の自動解決のみを記録する)。
fn cancel_pending_elicitations(
    elicitations: &ElicitationRegistry,
    events: Option<&mpsc::UnboundedSender<DriverEvent>>,
) {
    let mut registry = elicitations.lock().expect("elicitation registry poisoned");
    for (elicitation_id, pending) in registry.drain() {
        let _ = pending.sender.send(ElicitationOutcome::Cancel);
        if let Some(events) = events {
            let _ = events.send(DriverEvent::Event(
                UnifiedEventPayload::ElicitationResolved {
                    elicitation_id,
                    action: fxg_protocol::common::ElicitationAction::Cancel,
                    content: serde_json::Value::Null,
                    resolved_by: "system".to_owned(),
                },
            ));
        }
    }
}

/// `run_acp_process` の起動パラメータ。
struct AcpProcessTaskParams {
    /// 起動するエージェント
    agent: AcpAgent,
    /// プロセス ready (initialize 完了) 通知
    ready_tx: oneshot::Sender<anyhow::Result<()>>,
    /// プロセスへのコマンド受信
    control_rx: mpsc::UnboundedReceiver<ProcessCommand>,
}

/// ACP接続を確立し、アイドル待機とセッション処理を順に回す常駐タスク。
///
/// 1 プロセスにつき同時 1 セッションを担当し、セッション終了後は次の
/// `NewSession` まで待機する (プロセスの破棄はプールの TTL / `Dispose`)。
#[allow(clippy::too_many_lines)]
async fn run_acp_process(params: AcpProcessTaskParams) -> Result<(), agent_client_protocol::Error> {
    let AcpProcessTaskParams {
        agent,
        ready_tx,
        mut control_rx,
    } = params;
    let mut ready_tx = Some(ready_tx);
    let pty = Arc::new(fxg_pty::PtySessionManager::new());
    let terminals = Arc::new(TerminalRegistry::default());
    let permissions: PermissionRegistry = Arc::new(Mutex::new(HashMap::new()));
    let elicitations: ElicitationRegistry = Arc::new(Mutex::new(HashMap::new()));
    let contexts: SessionContexts = Arc::new(Mutex::new(HashMap::new()));

    // ハンドラ登録 (fs / 承認 / terminal)。
    // NOTE: `impl AsyncFnMut` を返すヘルパー経由では associated future の `Send` を
    // 証明できないため、ビルダーチェーンへクロージャを直接渡す。
    let builder = Client
        .builder()
        .name("fxg")
        .on_receive_request(
            {
                let permissions = Arc::clone(&permissions);
                let contexts = Arc::clone(&contexts);
                async move |request: RequestPermissionRequest, responder, cx| {
                    // セッションが既に閉じている (レース) 場合は安全側に倒して
                    // キャンセルで返す
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ));
                    };
                    let events = context.events.clone();
                    let request_id = responder.id().to_string();
                    let options = request
                        .options
                        .iter()
                        .map(|option| PermissionOption {
                            option_id: option.option_id.to_string(),
                            name: option.name.clone(),
                            kind: permission_kind_str(option.kind).to_owned(),
                        })
                        .collect::<Vec<_>>();
                    let tool = &request.tool_call.fields;
                    let summary = tool
                        .title
                        .clone()
                        .unwrap_or_else(|| request.tool_call.tool_call_id.to_string());

                    let _ = events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                        status: fxg_protocol::common::SessionStatus::WaitingPermission,
                        error_message: None,
                    }));
                    let _ =
                        events.send(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
                            request_id: request_id.clone(),
                            tool_name: tool.name.clone().unwrap_or_else(|| "tool".to_owned()),
                            summary,
                            options,
                            details: serde_json::to_value(&request.tool_call)
                                .unwrap_or(serde_json::Value::Null),
                        }));

                    let (decision_tx, decision_rx) = oneshot::channel();
                    permissions
                        .lock()
                        .expect("permission registry poisoned")
                        .insert(request_id, decision_tx);
                    let events_for_task = context.events.clone();
                    cx.spawn(async move {
                        let outcome = decision_rx
                            .await
                            .unwrap_or(RequestPermissionOutcome::Cancelled);
                        let _ = events_for_task.send(DriverEvent::Event(
                            UnifiedEventPayload::StatusChanged {
                                status: fxg_protocol::common::SessionStatus::Running,
                                error_message: None,
                            },
                        ));
                        responder.respond(RequestPermissionResponse::new(outcome))
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let contexts = Arc::clone(&contexts);
                async move |request: ReadTextFileRequest, responder, _cx| {
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond_with_internal_error("unknown session");
                    };
                    let path = resolve_path(&context.cwd, &request.path);
                    let result = (|| -> anyhow::Result<String> {
                        let content = std::fs::read_to_string(&path)
                            .with_context(|| format!("failed to read {}", path.display()))?;
                        if request.line.is_some() || request.limit.is_some() {
                            let start = request.line.unwrap_or(1).saturating_sub(1) as usize;
                            let take = request.limit.unwrap_or(u32::MAX) as usize;
                            Ok(content
                                .lines()
                                .skip(start)
                                .take(take)
                                .collect::<Vec<_>>()
                                .join("\n"))
                        } else {
                            Ok(content)
                        }
                    })();
                    match result {
                        Ok(content) => responder.respond(ReadTextFileResponse::new(content)),
                        Err(err) => responder.respond_with_internal_error(err.to_string()),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let contexts = Arc::clone(&contexts);
                async move |request: WriteTextFileRequest, responder, _cx| {
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond_with_internal_error("unknown session");
                    };
                    let path = resolve_path(&context.cwd, &request.path);
                    let old_text = std::fs::read_to_string(&path).unwrap_or_default();
                    match std::fs::write(&path, &request.content) {
                        Ok(()) => {
                            let path_string = path.to_string_lossy().into_owned();
                            let diff = build_file_diff(&path_string, &old_text, &request.content);
                            let _ = context.events.send(DriverEvent::Event(
                                UnifiedEventPayload::ToolCall {
                                    tool_call_id: format!("fs-{}", uuid_v7()),
                                    title: format!("write_text_file: {path_string}"),
                                    kind: "edit".to_owned(),
                                    status: "completed".to_owned(),
                                    locations: vec![path_string],
                                    diff: Some(diff),
                                    raw_output: None,
                                },
                            ));
                            responder.respond(WriteTextFileResponse::new())
                        }
                        Err(err) => responder.respond_with_internal_error(format!(
                            "failed to write {}: {err}",
                            path.display()
                        )),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pty = Arc::clone(&pty);
                let terminals = Arc::clone(&terminals);
                let contexts = Arc::clone(&contexts);
                async move |request: CreateTerminalRequest, responder, _cx| {
                    let Some(context) = session_context(&contexts, &request.session_id) else {
                        return responder.respond_with_internal_error("unknown session");
                    };
                    // `AsyncFnMut` は複数回呼ばれるため、キャプチャは clone して渡す
                    create_terminal(
                        pty.clone(),
                        terminals.clone(),
                        context.events.clone(),
                        request,
                        responder,
                    )
                    .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let terminals = Arc::clone(&terminals);
                async move |request: TerminalOutputRequest, responder, _cx| match terminals
                    .get(&request.terminal_id.to_string())
                {
                    Ok((_, _, output, exit_rx)) => {
                        let data = output.lock().expect("terminal output poisoned").clone();
                        let text = String::from_utf8_lossy(&data).into_owned();
                        let exit_status = exit_rx
                            .borrow()
                            .as_ref()
                            .copied()
                            .map(|code| exit_status_of(Some(code)));
                        responder.respond(
                            TerminalOutputResponse::new(text, false).exit_status(exit_status),
                        )
                    }
                    Err(err) => responder.respond_with_internal_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let terminals = Arc::clone(&terminals);
                async move |request: WaitForTerminalExitRequest, responder, cx| {
                    let Ok((_, _, _, mut exit_rx)) =
                        terminals.get(&request.terminal_id.to_string())
                    else {
                        return responder.respond_with_internal_error("unknown terminal");
                    };
                    cx.spawn(async move {
                        // 既に終了していれば現在値を返し、そうでなければ変更を待つ
                        if exit_rx.borrow().is_none() {
                            let _ = exit_rx.changed().await;
                        }
                        let status = exit_status_of(*exit_rx.borrow());
                        responder.respond(WaitForTerminalExitResponse::new(status))
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pty = Arc::clone(&pty);
                let terminals = Arc::clone(&terminals);
                async move |request: KillTerminalRequest, responder, _cx| match terminals
                    .get(&request.terminal_id.to_string())
                {
                    Ok((pty_id, ..)) => {
                        let _ = pty.kill(&pty_id);
                        responder.respond(KillTerminalResponse::new())
                    }
                    Err(err) => responder.respond_with_internal_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pty = Arc::clone(&pty);
                let terminals = Arc::clone(&terminals);
                async move |request: ReleaseTerminalRequest, responder, _cx| {
                    if let Some(state) = terminals.remove(&request.terminal_id.to_string()) {
                        let _ = pty.kill(&state.pty_id);
                    }
                    responder.respond(ReleaseTerminalResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let elicitations = Arc::clone(&elicitations);
                let contexts = Arc::clone(&contexts);
                async move |request: CreateElicitationRequest, responder, cx| {
                    // Phase 1 は form モードのみ対応 (capability で広告した範囲)。
                    // 未広告モードの要求は Invalid params で返す (spec)。
                    let (scope, schema) = match &request.mode {
                        ElicitationMode::Form(form) => (
                            form.scope.clone(),
                            serde_json::to_value(&form.requested_schema)
                                .unwrap_or(serde_json::Value::Null),
                        ),
                        other => {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params()
                                    .data(format!("unsupported elicitation mode: {other:?}")),
                            );
                        }
                    };
                    // セッションスコープのみ対応 (リクエストスコープは認証フロー用で未対応)
                    let ElicitationScope::Session(scope) = scope else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("request-scoped elicitation is not supported"),
                        );
                    };
                    let Some(context) = session_context(&contexts, &scope.session_id) else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params().data("unknown session"),
                        );
                    };
                    // form モードはプロトコル上の elicitation id を持たないため、
                    // JSON-RPC request id を elicitation id として使う (承認と同じ)。
                    let elicitation_id = responder.id().to_string();
                    let events = context.events.clone();
                    let _ = events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                        status: fxg_protocol::common::SessionStatus::WaitingInput,
                        error_message: None,
                    }));
                    let _ = events.send(DriverEvent::Event(
                        UnifiedEventPayload::ElicitationRequest {
                            elicitation_id: elicitation_id.clone(),
                            message: request.message.clone(),
                            mode: "form".to_owned(),
                            requested_schema: schema.clone(),
                            tool_call_id: scope.tool_call_id.as_ref().map(|id| id.to_string()),
                        },
                    ));

                    let (decision_tx, decision_rx) = oneshot::channel();
                    elicitations
                        .lock()
                        .expect("elicitation registry poisoned")
                        .insert(
                            elicitation_id,
                            PendingElicitation {
                                sender: decision_tx,
                                schema: Some(schema),
                            },
                        );
                    let events_for_task = context.events.clone();
                    cx.spawn(async move {
                        let outcome = decision_rx.await.unwrap_or(ElicitationOutcome::Cancel);
                        let _ = events_for_task.send(DriverEvent::Event(
                            UnifiedEventPayload::StatusChanged {
                                status: fxg_protocol::common::SessionStatus::Running,
                                error_message: None,
                            },
                        ));
                        let response = match outcome {
                            ElicitationOutcome::Accept(content) => {
                                CreateElicitationResponse::new(AcpElicitationAction::Accept(
                                    ElicitationAcceptAction::new().content(content),
                                ))
                            }
                            ElicitationOutcome::Decline => {
                                CreateElicitationResponse::new(AcpElicitationAction::Decline)
                            }
                            ElicitationOutcome::Cancel => {
                                CreateElicitationResponse::new(AcpElicitationAction::Cancel)
                            }
                        };
                        responder.respond(response)
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CompleteElicitationNotification, _cx| {
                // Phase 1 は form モードのみのため、URL モードの完了通知は
                // 未知 ID として無視する (spec: 未知 ID は無視しなければならない)。
                tracing::debug!(
                    elicitation_id = %notification.elicitation_id,
                    "elicitation/complete notification ignored (url mode not supported)"
                );
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        );

    // 外部エージェントプロセスの起動とプロセスツリー管理 (Windows: Job Object)
    let (child_stdin, child_stdout, child_stderr, mut child) = match agent.spawn_process() {
        Ok(parts) => parts,
        Err(err) => {
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(Err(anyhow!("failed to start agent process: {err}")));
            }
            return Err(err);
        }
    };

    let _process_guard = match fxg_pty::ProcessTreeGuard::new() {
        Ok(guard) => guard,
        Err(err) => {
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(Err(anyhow!("failed to create process tree guard: {err}")));
            }
            return Err(agent_client_protocol::Error::internal_error().data(err.to_string()));
        }
    };
    // エージェントのプロセスツリーを終了対象として登録する
    // (プラットフォーム差は `fxg_pty` 側に集約)。失敗しても起動は継続するが、
    // 終了時のツリー kill が効かなくなるため警告する。
    if let Err(err) = _process_guard.attach_pid(child.id()) {
        tracing::warn!("failed to attach acp agent process tree to guard: {err}");
    }

    // エージェント stderr をデーモンのログへ流す (パイプ詰まり防止)
    spawn_acp_stderr_pump(child_stderr);

    // クロージャ (async move) が `permissions` / `elicitations` をムーブするため、
    // teardown 用に複製を保持しておく
    let permissions_for_teardown = Arc::clone(&permissions);
    let elicitations_for_teardown = Arc::clone(&elicitations);
    let transport = ByteStreams::new(child_stdin, child_stdout);
    let run_result = builder
        .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
            // 1) initialize (プロセスにつき 1 回。fs 読み書き + terminal +
            //    elicitation (form) をサポート宣言)
            let capabilities = ClientCapabilities::new()
                .fs(FileSystemCapabilities::new()
                    .read_text_file(true)
                    .write_text_file(true))
                .terminal(true)
                .elicitation(
                    ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()),
                );
            let initialize = match cx
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_capabilities(capabilities)
                        .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
                )
                .block_task()
                .await
            {
                Ok(initialize) => initialize,
                Err(err) => {
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(Err(anyhow!("acp initialize failed: {err}")));
                    }
                    return Err(err);
                }
            };
            let agent_capabilities = initialize.agent_capabilities.clone();
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(Ok(()));
            }

            // 2) セッション処理ループ。1 プロセスにつき同時 1 セッションを担当し、
            //    セッション終了後は次の `NewSession` までアイドル待機する。
            while let Some(command) = control_rx.recv().await {
                let ProcessCommand::NewSession {
                    params,
                    events,
                    lease,
                    commands: (cmd_tx, cmd_rx),
                    ready,
                } = command
                else {
                    // Dispose = プロセス終了
                    break;
                };
                // セッション構築。失敗した場合は接続の健全性が不明なため、
                // エラーを返した上でプロセスを畳む (呼び出し側はコールド
                // スタートへ回帰する)。
                let (session, acp_session_id, context_restored, capabilities_state) =
                    match build_session(&cx, &agent_capabilities, &params, &events, &contexts).await
                    {
                        Ok(built) => built,
                        Err(err) => {
                            let _ = ready.send(Err(err));
                            lease.mark_dispose();
                            break;
                        }
                    };
                let session_end_tx = lease.take_session_end_sender();
                let handle = AcpSessionHandle {
                    acp_session_id: acp_session_id.clone(),
                    conn: cx.clone(),
                    commands: cmd_tx,
                    permissions: Arc::clone(&permissions),
                    elicitations: Arc::clone(&elicitations),
                    events: Mutex::new(Some(events.clone())),
                    pty: Arc::clone(&pty),
                    lease: Arc::clone(&lease),
                };
                let _ = ready.send(Ok((handle, context_restored)));

                // 3) ターンループ (プロンプト送信と更新読み取りを直列化)
                let outcome = serve_session(session, cmd_rx, &events, capabilities_state).await;
                contexts
                    .lock()
                    .expect("session contexts poisoned")
                    .remove(&acp_session_id.to_string());
                if let Some(tx) = session_end_tx {
                    let _ = tx.send(());
                }
                // セッションに紐づく未解決の承認・質問はキャンセルで閉じる
                cancel_pending_permissions(&permissions);
                cancel_pending_elicitations(&elicitations, Some(&events));
                match outcome {
                    ServeOutcome::Closed => {
                        let _ =
                            events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                                status: fxg_protocol::common::SessionStatus::Stopped,
                                error_message: None,
                            }));
                    }
                    ServeOutcome::Fatal(err) => {
                        tracing::warn!(
                            session_id = %params.session_id,
                            "acp session ended: {err:#}"
                        );
                        let _ = events.send(DriverEvent::Failed {
                            message: err.to_string(),
                        });
                        // 接続異常時はプロセスを畳む (プールへは返さない)
                        lease.mark_dispose();
                        break;
                    }
                }
            }

            // ループを抜ける = プロセスシャットダウン
            Ok(())
        })
        .await;

    // 接続完了後、プロセスツリーを確実に終了させる。
    //
    // ラッパー型ランチャー (PyInstaller onefile 等) は「内側のプロセスが終了すると
    // 自身で後始末 (一時ファイルの削除等) をしてから自然終了する」ため、
    // 内側から順に終了してルートの自然終了を待つ。
    // プラットフォーム差は `fxg_pty` 側に集約されている。
    match _process_guard
        .shutdown_tree(child.id(), AGENT_EXIT_GRACE)
        .await
    {
        Ok(outcome) => tracing::debug!(?outcome, "acp agent process tree shut down"),
        Err(err) => tracing::warn!("failed to shut down acp agent process tree: {err}"),
    }
    match child.status().await {
        Ok(status) => tracing::debug!(?status, "acp agent process exited"),
        Err(err) => tracing::debug!("failed to reap acp agent process: {err}"),
    }

    // 未解決の承認・質問が残っていればキャンセルで閉じる (プロセス終了)
    cancel_pending_permissions(&permissions_for_teardown);
    cancel_pending_elicitations(&elicitations_for_teardown, None);

    run_result
}

/// ターンループの終了理由。
enum ServeOutcome {
    /// 正常終了 (`AcpCommand::Shutdown`)
    Closed,
    /// 接続・ターンの異常 (プロセスを畳む)
    Fatal(anyhow::Error),
}

/// セッションを作成・初期化する (`session/new` | `session/resume` | `session/load`)。
///
/// セッション情報を [`SessionContexts`] へ登録し (fs / terminal ハンドラが参照)、
/// 初期モード適用・モード/コマンド/設定項目の配信まで行う。
#[allow(clippy::type_complexity)]
async fn build_session(
    cx: &ConnectionTo<Agent>,
    agent_capabilities: &AgentCapabilities,
    params: &SessionParams,
    events: &mpsc::UnboundedSender<DriverEvent>,
    contexts: &SessionContexts,
) -> anyhow::Result<(
    ActiveSession<'static, Agent>,
    SessionId,
    bool,
    SessionCapabilitiesState,
)> {
    // セッション取得 (新規作成 / session/resume / session/load)
    let plan = plan_resume(agent_capabilities, params.resume.as_ref())?;
    let (session, context_restored) = match acquire_session(cx, plan, &params.cwd).await {
        Ok(acquired) => acquired,
        // ネイティブ限定の再開 (`allow_fresh = false`) では、復元失敗は
        // 「ネイティブ復元不可」として呼び出し側 (ノード) に伝える
        Err(err)
            if params
                .resume
                .as_ref()
                .is_some_and(|resume| !resume.allow_fresh) =>
        {
            return Err(
                NativeResumeUnavailable(format!("failed to resume acp session: {err}")).into(),
            );
        }
        Err(err) => return Err(anyhow!("failed to create acp session: {err}")),
    };
    let acp_session_id = session.session_id().clone();
    // セッション情報を登録する (fs / terminal ハンドラが参照する)
    contexts.lock().expect("session contexts poisoned").insert(
        acp_session_id.to_string(),
        SessionContext {
            cwd: params.cwd.clone(),
            events: events.clone(),
        },
    );
    let _ = events.send(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
        agent_session_id: acp_session_id.to_string(),
    }));

    // 初期モード適用
    if let Some(mode_id) = params.initial_mode.as_ref()
        && let Err(err) = cx
            .send_request(SetSessionModeRequest::new(
                acp_session_id.clone(),
                mode_id.clone(),
            ))
            .block_task()
            .await
    {
        tracing::warn!("failed to set initial mode {mode_id}: {err}");
    }

    // モード・スラッシュコマンド・設定項目を配信
    let (modes, commands, config_options) = session_capabilities(&session);
    let current_mode = session
        .modes()
        .map(|state| state.current_mode_id.to_string());
    let capabilities_state = SessionCapabilitiesState {
        current_mode: current_mode.clone(),
        available_modes: modes.clone(),
        available_commands: commands.clone(),
        config_options: config_options.clone(),
    };
    if !modes.is_empty()
        || !commands.is_empty()
        || !config_options.is_empty()
        || current_mode.is_some()
    {
        let _ = events.send(DriverEvent::Event(capabilities_state.to_payload()));
    }
    let _ = events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
        status: fxg_protocol::common::SessionStatus::Idle,
        error_message: None,
    }));

    Ok((
        session,
        acp_session_id,
        context_restored,
        capabilities_state,
    ))
}

/// 1 セッション分のターンループ (プロンプト送信と更新読み取りを直列化)。
async fn serve_session(
    mut session: ActiveSession<'static, Agent>,
    mut cmd_rx: mpsc::UnboundedReceiver<AcpCommand>,
    events: &mpsc::UnboundedSender<DriverEvent>,
    mut capabilities_state: SessionCapabilitiesState,
) -> ServeOutcome {
    let mut accumulator = StreamingAccumulator::default();
    while let Some(command) = cmd_rx.recv().await {
        match command {
            AcpCommand::Prompt(text) => {
                accumulator.reset();
                let _ = events.send(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: fxg_protocol::common::SessionStatus::Running,
                    error_message: None,
                }));
                if let Err(err) = session.send_prompt(text) {
                    return ServeOutcome::Fatal(err.into());
                }
                loop {
                    match session.read_update().await {
                        Ok(SessionMessage::StopReason(reason)) => {
                            for payload in accumulator.finish_all() {
                                let _ = events.send(DriverEvent::Event(payload));
                            }
                            tracing::debug!(?reason, "acp turn finished");
                            let _ = events.send(DriverEvent::Event(
                                UnifiedEventPayload::StatusChanged {
                                    status: fxg_protocol::common::SessionStatus::Idle,
                                    error_message: None,
                                },
                            ));
                            break;
                        }
                        Ok(SessionMessage::SessionMessage(dispatch)) => {
                            let acc = &mut accumulator;
                            let ev = events;
                            let caps = &mut capabilities_state;
                            let result = MatchDispatch::new(dispatch)
                                .if_notification(async move |notification: SessionNotification| {
                                    for event in map_session_update(notification.update, acc, caps)
                                    {
                                        let _ = ev.send(event);
                                    }
                                    Ok(())
                                })
                                .await
                                .otherwise_ignore();
                            if let Err(err) = result {
                                return ServeOutcome::Fatal(err.into());
                            }
                        }
                        // `SessionMessage` は #[non_exhaustive]
                        Ok(_) => {}
                        Err(err) => return ServeOutcome::Fatal(err.into()),
                    }
                }
            }
            AcpCommand::Shutdown => break,
        }
    }
    ServeOutcome::Closed
}

/// エージェントプロセスの stderr をデーモンのログ (`debug`) へ流す。
fn spawn_acp_stderr_pump<R>(reader: R)
where
    R: futures_util::io::AsyncRead + Unpin + Send + 'static,
{
    use futures_util::AsyncBufReadExt;
    use futures_util::StreamExt;
    tokio::spawn(async move {
        let mut lines = futures_util::io::BufReader::new(reader).lines();
        while let Some(Ok(line)) = lines.next().await {
            tracing::debug!(target: "fxg_acp::acp", "acp agent: {line}");
        }
    });
}

/// セッションのモード・コマンド・設定項目を取り出す。
fn session_capabilities(
    session: &ActiveSession<'static, Agent>,
) -> (Vec<ModeInfo>, Vec<CommandInfo>, Vec<ConfigOptionInfo>) {
    let modes = session
        .modes()
        .map(|state| {
            state
                .available_modes
                .iter()
                .map(|mode| ModeInfo {
                    mode_id: mode.id.to_string(),
                    name: mode.name.clone(),
                    description: mode.description.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let config_options = session
        .config_options()
        .map(|options| options.iter().map(map_config_option).collect())
        .unwrap_or_default();
    // スラッシュコマンドは session/update (AvailableCommandsUpdate) で届くため
    // ここでは空 (以降の通知で更新される)
    (modes, Vec::new(), config_options)
}

/// `resume` 指定とエージェント capability からセッション取得方法を決める。
#[derive(Debug)]
enum ResumePlan {
    /// `session/resume` (履歴 Replay なし)
    Resume(String),
    /// `session/load` (Replay 通知は破棄する)
    Load(String),
    /// 新規セッション作成 (呼び出し側が履歴 Replay を注入する)
    Fresh,
}

/// セッション取得方法を決定する。
///
/// fxg は会話履歴の正本を自前のイベントログに持つため、履歴を送り返さない
/// `session/resume` を優先する。resume 非対応の場合のみ `session/load`
/// (Replay 通知は破棄) を使い、どちらも無ければ新規作成する
/// (設計: `docs/04-agent-drivers-and-windows.md` §4.3)。
///
/// `resume.allow_fresh` が `false` の場合 (送信時の自動レジューム) は
/// 新規作成せず [`NativeResumeUnavailable`] を返し、呼び出し側に履歴 Replay
/// での明示的な再開を促す。
fn plan_resume(
    capabilities: &AgentCapabilities,
    resume: Option<&ResumeRequest>,
) -> anyhow::Result<ResumePlan> {
    // 再開指定なし → 新規作成
    let Some(resume) = resume else {
        return Ok(ResumePlan::Fresh);
    };
    let Some(agent_session_id) = resume.agent_session_id.clone() else {
        // エージェント側ID未記録 (呼び出し側の履歴 Replay 注入前提)
        return if resume.allow_fresh {
            Ok(ResumePlan::Fresh)
        } else {
            Err(NativeResumeUnavailable("no agent session id recorded".to_owned()).into())
        };
    };

    if capabilities.session_capabilities.resume.is_some() {
        return Ok(ResumePlan::Resume(agent_session_id));
    }
    if capabilities.load_session {
        return Ok(ResumePlan::Load(agent_session_id));
    }
    if resume.allow_fresh {
        return Ok(ResumePlan::Fresh);
    }
    Err(NativeResumeUnavailable(format!(
        "agent does not support session/resume or session/load (agent_session_id={agent_session_id})"
    ))
    .into())
}

/// セッションを取得する ([`ResumePlan`] に従う)。
///
/// 戻り値の `bool` はネイティブ復元の成否 (`false` = 新規作成。呼び出し側が
/// 履歴 Replay を注入する)。
async fn acquire_session(
    cx: &ConnectionTo<Agent>,
    plan: ResumePlan,
    cwd: &Path,
) -> Result<(ActiveSession<'static, Agent>, bool), agent_client_protocol::Error> {
    match plan {
        ResumePlan::Fresh => {
            let session = cx
                .build_session_from(NewSessionRequest::new(cwd))
                .block_task()
                .start_session()
                .await?;
            Ok((session, false))
        }
        ResumePlan::Resume(agent_session_id) => {
            let restored = cx
                .resume_session(agent_session_id, cwd)
                .block_task()
                .start_session()
                .await?;
            let (session, _response) = restored.into_parts();
            Ok((session, true))
        }
        ResumePlan::Load(agent_session_id) => {
            let restored = cx
                .load_session(agent_session_id, cwd)
                .block_task()
                .start_session()
                .await?;
            let (mut session, _response) = restored.into_parts();
            discard_replay_updates(&mut session);
            Ok((session, true))
        }
    }
}

/// `session/load` の応答前に届いた Replay 通知を破棄する。
///
/// ACP 仕様では会話履歴の Replay は load 応答前に完了し、SDK はその間の通知を
/// 復元済みセッションにバッファする。fxg は履歴の正本を自前のイベントログに
/// 持つため、二重記録を避けるべく応答直後にキューを drain して捨てる。
fn discard_replay_updates(session: &mut ActiveSession<'static, Agent>) {
    use futures_util::FutureExt;

    let mut discarded = 0usize;
    while let Some(result) = session.read_update().now_or_never() {
        if result.is_err() {
            break;
        }
        discarded += 1;
    }
    if discarded > 0 {
        tracing::debug!(discarded, "discarded session/load replay updates");
    }
}

/// ACP の設定項目を FXG の表示用型へ変換する。
fn map_config_option(option: &SessionConfigOption) -> ConfigOptionInfo {
    let (current_value, options) = match &option.kind {
        SessionConfigKind::Select(select) => (
            serde_json::Value::String(select.current_value.to_string()),
            match &select.options {
                agent_client_protocol::schema::v1::SessionConfigSelectOptions::Ungrouped(
                    values,
                ) => values
                    .iter()
                    .map(|value| serde_json::Value::String(value.value.to_string()))
                    .collect(),
                agent_client_protocol::schema::v1::SessionConfigSelectOptions::Grouped(groups) => {
                    groups
                        .iter()
                        .flat_map(|group| &group.options)
                        .map(|value| serde_json::Value::String(value.value.to_string()))
                        .collect()
                }
                _ => Vec::new(),
            },
        ),
        SessionConfigKind::Boolean(boolean) => (
            serde_json::Value::Bool(boolean.current_value),
            vec![
                serde_json::Value::Bool(true),
                serde_json::Value::Bool(false),
            ],
        ),
        _ => (serde_json::Value::Null, Vec::new()),
    };
    ConfigOptionInfo {
        key: option.id.to_string(),
        name: option.name.clone(),
        current_value,
        options,
    }
}

/// セッションの能力情報 (モード・コマンド・設定項目) の追跡状態。
#[derive(Debug, Default, Clone)]
struct SessionCapabilitiesState {
    current_mode: Option<String>,
    available_modes: Vec<ModeInfo>,
    available_commands: Vec<CommandInfo>,
    config_options: Vec<ConfigOptionInfo>,
}

impl SessionCapabilitiesState {
    fn to_payload(&self) -> UnifiedEventPayload {
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: self.current_mode.clone(),
            available_modes: self.available_modes.clone(),
            available_commands: self.available_commands.clone(),
            config_options: self.config_options.clone(),
        }
    }
}

/// ストリーミングチャンクを蓄積し、メッセージ切り替え時に完成イベントを作る。
#[derive(Debug, Default)]
struct StreamingAccumulator {
    message: Option<(String, String)>,
    thought: Option<(String, String)>,
}

impl StreamingAccumulator {
    fn reset(&mut self) {
        self.message = None;
        self.thought = None;
    }

    /// メッセージチャンクを追加する。メッセージIDが変わった場合は
    /// 直前のメッセージの完成イベントを返す。
    fn push_message(&mut self, message_id: String, delta: &str) -> Option<UnifiedEventPayload> {
        match &mut self.message {
            Some((id, text)) if *id == message_id => {
                text.push_str(delta);
                None
            }
            Some((id, text)) => {
                let finished = UnifiedEventPayload::AgentMessage {
                    message_id: id.clone(),
                    text: std::mem::take(text),
                    is_complete: true,
                };
                self.message = Some((message_id, delta.to_owned()));
                Some(finished)
            }
            None => {
                self.message = Some((message_id, delta.to_owned()));
                None
            }
        }
    }

    /// 思考チャンクを追加する (メッセージと同様の挙動)。
    fn push_thought(&mut self, thought_id: String, delta: &str) -> Option<UnifiedEventPayload> {
        match &mut self.thought {
            Some((id, text)) if *id == thought_id => {
                text.push_str(delta);
                None
            }
            Some((id, text)) => {
                let finished = UnifiedEventPayload::AgentThought {
                    thought_id: id.clone(),
                    text: std::mem::take(text),
                    is_complete: true,
                };
                self.thought = Some((thought_id, delta.to_owned()));
                Some(finished)
            }
            None => {
                self.thought = Some((thought_id, delta.to_owned()));
                None
            }
        }
    }

    /// 蓄積中のメッセージ・思考を完成イベントとして取り出す。
    fn finish_all(&mut self) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        if let Some((id, text)) = self.message.take() {
            out.push(UnifiedEventPayload::AgentMessage {
                message_id: id,
                text,
                is_complete: true,
            });
        }
        if let Some((id, text)) = self.thought.take() {
            out.push(UnifiedEventPayload::AgentThought {
                thought_id: id,
                text,
                is_complete: true,
            });
        }
        out
    }
}

/// `session/update` 通知を [`DriverEvent`] 列へ変換する。
fn map_session_update(
    update: SessionUpdate,
    accumulator: &mut StreamingAccumulator,
    capabilities: &mut SessionCapabilitiesState,
) -> Vec<DriverEvent> {
    let mut events = Vec::new();
    match update {
        SessionUpdate::UserMessageChunk(_) => {
            // ユーザーメッセージはマネージャが `UserMessage` として記録済み
        }
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content,
            message_id,
            ..
        }) => {
            if let ContentBlock::Text(TextContent { text, .. }) = content {
                let message_id = message_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "acp-message".to_owned());
                if let Some(finished) = accumulator.push_message(message_id.clone(), &text) {
                    events.push(DriverEvent::Event(finished));
                }
                events.push(DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta {
                    message_id,
                    text_delta: text,
                }));
            }
        }
        SessionUpdate::AgentThoughtChunk(ContentChunk {
            content,
            message_id,
            ..
        }) => {
            if let ContentBlock::Text(TextContent { text, .. }) = content {
                let thought_id = message_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "acp-thought".to_owned());
                if let Some(finished) = accumulator.push_thought(thought_id.clone(), &text) {
                    events.push(DriverEvent::Event(finished));
                }
                events.push(DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta {
                    thought_id,
                    text_delta: text,
                }));
            }
        }
        SessionUpdate::ToolCall(tool_call) => {
            events.push(DriverEvent::Event(tool_call_event(&tool_call)));
        }
        SessionUpdate::ToolCallUpdate(update) => {
            events.push(DriverEvent::Event(tool_call_update_event(&update)));
        }
        SessionUpdate::Plan(plan) => {
            let entries = plan
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| PlanEntry {
                    id: format!("plan-{index}"),
                    title: entry.content.clone(),
                    status: plan_status_str(&entry.status).to_owned(),
                })
                .collect();
            events.push(DriverEvent::Event(UnifiedEventPayload::PlanUpdate {
                entries,
            }));
        }
        SessionUpdate::AvailableCommandsUpdate(update) => {
            let commands = update
                .available_commands
                .iter()
                .map(|command| CommandInfo {
                    name: command.name.clone(),
                    description: command.description.clone(),
                    input_hint: command.input.as_ref().map(|input| match input {
                        agent_client_protocol::schema::v1::AvailableCommandInput::Unstructured(
                            unstructured,
                        ) => unstructured.hint.clone(),
                        _ => String::new(),
                    }),
                })
                .collect();
            capabilities.available_commands = commands;
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::CurrentModeUpdate(update) => {
            capabilities.current_mode = Some(update.current_mode_id.to_string());
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::ConfigOptionUpdate(update) => {
            capabilities.config_options = update
                .config_options
                .iter()
                .map(map_config_option)
                .collect();
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::SessionInfoUpdate(update) => {
            if let Some(title) = update.title.value() {
                let trimmed = title.trim();
                if !trimmed.is_empty() {
                    events.push(DriverEvent::Event(
                        UnifiedEventPayload::SessionTitleChanged {
                            title: trimmed.to_owned(),
                        },
                    ));
                }
            }
        }
        // UsageUpdate 等は Phase 3 では未対応
        other => {
            tracing::trace!("unhandled session update: {other:?}");
        }
    }
    events
}

/// ACP の `ToolCall` を正規化イベントへ変換する。
fn tool_call_event(tool_call: &ToolCall) -> UnifiedEventPayload {
    let (diff, _) = tool_call_contents(&tool_call.content);
    UnifiedEventPayload::ToolCall {
        tool_call_id: tool_call.tool_call_id.to_string(),
        title: tool_call.title.clone(),
        kind: tool_kind_str(tool_call.kind).to_owned(),
        status: tool_status_str(tool_call.status).to_owned(),
        locations: tool_call
            .locations
            .iter()
            .map(|location| location.path.to_string_lossy().into_owned())
            .collect(),
        diff,
        raw_output: tool_call.raw_output.as_ref().map(|value| value.to_string()),
    }
}

/// ACP の `ToolCallUpdate` を正規化イベントへ変換する (同一 `tool_call_id` への追記)。
fn tool_call_update_event(update: &ToolCallUpdate) -> UnifiedEventPayload {
    let fields = &update.fields;
    let (diff, locations) = fields
        .content
        .as_ref()
        .map(|content| tool_call_contents(content))
        .unwrap_or((None, Vec::new()));
    let locations = fields
        .locations
        .as_ref()
        .map(|values| {
            values
                .iter()
                .map(|location| location.path.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or(locations);
    UnifiedEventPayload::ToolCall {
        tool_call_id: update.tool_call_id.to_string(),
        title: fields.title.clone().unwrap_or_default(),
        kind: fields
            .kind
            .map(|kind| tool_kind_str(kind).to_owned())
            .unwrap_or_default(),
        status: fields
            .status
            .map(|status| tool_status_str(status).to_owned())
            .unwrap_or_default(),
        locations,
        diff,
        raw_output: fields.raw_output.as_ref().map(|value| value.to_string()),
    }
}

/// `ToolCallContent` 列から Diff とファイルパス一覧を取り出す。
fn tool_call_contents(contents: &[ToolCallContent]) -> (Option<FileDiff>, Vec<String>) {
    let mut diff = None;
    let mut locations = Vec::new();
    for content in contents {
        if let ToolCallContent::Diff(acp_diff) = content {
            let path = acp_diff.path.to_string_lossy().into_owned();
            locations.push(path.clone());
            let old_text = acp_diff.old_text.clone().unwrap_or_default();
            diff = Some(build_file_diff(&path, &old_text, &acp_diff.new_text));
        }
    }
    (diff, locations)
}

/// 変更前後のテキストから Unified Diff を計算して [`FileDiff`] を作る。
pub(crate) fn build_file_diff(path: &str, old_text: &str, new_text: &str) -> FileDiff {
    let patch = diffy::create_patch(old_text, new_text);
    let unified_diff = patch.to_string();
    let mut additions = 0u32;
    let mut deletions = 0u32;
    for line in unified_diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            additions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }
    FileDiff {
        path: path.to_owned(),
        old_text: Some(old_text.to_owned()),
        new_text: Some(new_text.to_owned()),
        unified_diff,
        additions,
        deletions,
    }
}

fn tool_kind_str(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move => "edit",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        _ => "other",
    }
}

fn tool_status_str(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Pending => "pending",
        ToolCallStatus::InProgress => "in_progress",
        ToolCallStatus::Completed => "completed",
        ToolCallStatus::Failed => "failed",
        _ => "pending",
    }
}

fn plan_status_str(status: &agent_client_protocol::schema::v1::PlanEntryStatus) -> &'static str {
    use agent_client_protocol::schema::v1::PlanEntryStatus;
    match status {
        PlanEntryStatus::Pending => "pending",
        PlanEntryStatus::InProgress => "in_progress",
        PlanEntryStatus::Completed => "completed",
        _ => "pending",
    }
}

fn permission_kind_str(kind: PermissionOptionKind) -> &'static str {
    match kind {
        PermissionOptionKind::AllowOnce => "allow_once",
        PermissionOptionKind::AllowAlways => "allow_always",
        PermissionOptionKind::RejectOnce => "reject_once",
        PermissionOptionKind::RejectAlways => "reject_always",
        _ => "unknown",
    }
}

// ----------------------------------------------------------------------
// ハンドラ補助 (fs / terminal)
// ----------------------------------------------------------------------

/// パスをセッション作業ディレクトリ基準で解決する。
fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// `accept` の content を `requested_schema` に対して軽量検証する。
///
/// ACP の elicitation が許す制限スキーマ (object / string (enum, oneOf) /
/// integer / number / boolean / string array) のみを検証する (仕様上クライアント
/// SHOULD。エージェントも再検証する)。既知の型以外 (将来拡張) は素通しする。
fn validate_elicitation_content(
    schema: &serde_json::Value,
    content: &serde_json::Value,
) -> Result<(), String> {
    let Some(content) = content.as_object() else {
        return Err("content must be an object".to_owned());
    };
    let Some(properties) = schema.get("properties").and_then(|value| value.as_object()) else {
        // properties を持たない schema では形状検証しない
        return Ok(());
    };
    if let Some(required) = schema.get("required").and_then(|value| value.as_array()) {
        for key in required.iter().filter_map(|value| value.as_str()) {
            if !content.contains_key(key) {
                return Err(format!("missing required property: {key}"));
            }
        }
    }
    for (key, value) in content {
        let Some(property) = properties.get(key) else {
            return Err(format!("unexpected property: {key}"));
        };
        validate_elicitation_property(key, property, value)?;
    }
    Ok(())
}

/// elicitation の制限スキーマにおける 1 プロパティの型検証。
fn validate_elicitation_property(
    key: &str,
    property: &serde_json::Value,
    value: &serde_json::Value,
) -> Result<(), String> {
    match property.get("type").and_then(|value| value.as_str()) {
        Some("string") => {
            let Some(text) = value.as_str() else {
                return Err(format!("property {key} must be a string"));
            };
            if let Some(values) = property.get("enum").and_then(|value| value.as_array())
                && !values
                    .iter()
                    .any(|candidate| candidate.as_str() == Some(text))
            {
                return Err(format!("property {key} is not one of the allowed values"));
            }
            if let Some(options) = property.get("oneOf").and_then(|value| value.as_array())
                && !options.iter().any(|option| {
                    option.get("const").and_then(|value| value.as_str()) == Some(text)
                })
            {
                return Err(format!("property {key} is not one of the allowed options"));
            }
        }
        Some("integer") => {
            if !value.is_i64() && !value.is_u64() {
                return Err(format!("property {key} must be an integer"));
            }
        }
        Some("number") => {
            if !value.is_number() {
                return Err(format!("property {key} must be a number"));
            }
        }
        Some("boolean") => {
            if !value.is_boolean() {
                return Err(format!("property {key} must be a boolean"));
            }
        }
        Some("array") => {
            let Some(items) = value.as_array() else {
                return Err(format!("property {key} must be an array"));
            };
            if !items.iter().all(|item| item.is_string()) {
                return Err(format!("property {key} must be an array of strings"));
            }
        }
        // 既知の制限スキーマ外 (将来拡張) は検証しない
        _ => {}
    }
    Ok(())
}

// ----------------------------------------------------------------------
// terminal/* (fxg-pty)
// ----------------------------------------------------------------------

/// ACP terminal の実行状態。
struct TerminalState {
    pty_id: String,
    command: String,
    output: Arc<Mutex<Vec<u8>>>,
    exit_rx: watch::Receiver<Option<i32>>,
}

/// ACP terminal レジストリ。
#[derive(Default)]
struct TerminalRegistry {
    terminals: Mutex<HashMap<String, TerminalState>>,
}

/// `TerminalRegistry::get` の戻り値 (pty_id, command, output, exit_rx)。
type TerminalSnapshot = (
    String,
    String,
    Arc<Mutex<Vec<u8>>>,
    watch::Receiver<Option<i32>>,
);

impl TerminalRegistry {
    fn get(&self, terminal_id: &str) -> Result<TerminalSnapshot, String> {
        let terminals = self.terminals.lock().expect("terminal registry poisoned");
        let state = terminals
            .get(terminal_id)
            .ok_or_else(|| format!("unknown terminal: {terminal_id}"))?;
        Ok((
            state.pty_id.clone(),
            state.command.clone(),
            Arc::clone(&state.output),
            state.exit_rx.clone(),
        ))
    }

    fn remove(&self, terminal_id: &str) -> Option<TerminalState> {
        self.terminals
            .lock()
            .expect("terminal registry poisoned")
            .remove(terminal_id)
    }
}

/// `terminal/create`: ACP ターミナルを fxg-pty で起動する。
async fn create_terminal(
    pty: Arc<fxg_pty::PtySessionManager>,
    terminals: Arc<TerminalRegistry>,
    events: mpsc::UnboundedSender<DriverEvent>,
    request: CreateTerminalRequest,
    responder: agent_client_protocol::Responder<CreateTerminalResponse>,
) -> Result<(), agent_client_protocol::Error> {
    let terminal_id = uuid_v7();
    let mut spawn = fxg_pty::PtySpawnRequest::new(terminal_id.clone());
    spawn.shell_cmd = Some(request.command.clone());
    spawn.args = request.args.clone();
    spawn.cwd = request.cwd.clone();
    spawn.env = request
        .env
        .iter()
        .map(|variable| (variable.name.clone(), variable.value.clone()))
        .collect();
    let command_line = format!("{} {}", request.command, request.args.join(" "));

    match pty.spawn(spawn) {
        Ok(_) => {
            let Ok(mut receiver) = pty.subscribe(&terminal_id) else {
                return responder.respond_with_internal_error("failed to subscribe terminal");
            };
            let output: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
            let (exit_tx, exit_rx) = watch::channel(None);
            let limit = request.output_byte_limit.map(|limit| limit as usize);
            // PTY 出力の収集タスク (差分配信 + 終了時の完成イベント)
            let output_for_task = Arc::clone(&output);
            let events_for_task = events.clone();
            let command_for_task = command_line.clone();
            let terminal_id_for_task = terminal_id.clone();
            tokio::spawn(async move {
                loop {
                    match receiver.recv().await {
                        Ok(fxg_pty::PtyEvent::Output { data, .. }) => {
                            {
                                let mut buffer =
                                    output_for_task.lock().expect("terminal output poisoned");
                                buffer.extend_from_slice(&data);
                                if let Some(limit) = limit
                                    && buffer.len() > limit
                                {
                                    let excess = buffer.len() - limit;
                                    buffer.drain(..excess);
                                }
                            }
                            let _ = events_for_task.send(DriverEvent::Delta(
                                StreamDeltaPayload::TerminalOutputDelta {
                                    terminal_id: terminal_id_for_task.clone(),
                                    data_b64: base64_encode(&data),
                                },
                            ));
                        }
                        Ok(fxg_pty::PtyEvent::Exit { exit_code, .. }) => {
                            let data = std::mem::take(
                                &mut *output_for_task.lock().expect("terminal output poisoned"),
                            );
                            let _ = events_for_task.send(DriverEvent::Event(
                                UnifiedEventPayload::TerminalOutput {
                                    terminal_id: terminal_id_for_task.clone(),
                                    command: command_for_task.clone(),
                                    data_b64: base64_encode(&data),
                                    exit_code,
                                },
                            ));
                            let _ = exit_tx.send(exit_code);
                            break;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            terminals
                .terminals
                .lock()
                .expect("terminal registry poisoned")
                .insert(
                    terminal_id.clone(),
                    TerminalState {
                        pty_id: terminal_id.clone(),
                        command: command_line,
                        output,
                        exit_rx,
                    },
                );
            responder.respond(CreateTerminalResponse::new(TerminalId::new(terminal_id)))
        }
        Err(err) => {
            responder.respond_with_internal_error(format!("failed to spawn terminal: {err}"))
        }
    }
}

/// 終了コードから ACP の `TerminalExitStatus` を作る。
fn exit_status_of(code: Option<i32>) -> TerminalExitStatus {
    let mut status = TerminalExitStatus::new();
    if let Some(code) = code.and_then(|code| u32::try_from(code).ok()) {
        status = status.exit_code(code);
    }
    status
}

/// Base64 エンコード (改行なし)。
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{Plan, PlanEntry as AcpPlanEntry, PlanEntryPriority};

    fn text_chunk(text: &str, message_id: Option<&str>) -> SessionUpdate {
        // `ContentChunk` は non_exhaustive のため JSON 経由で構築する
        let mut value = serde_json::json!({
            "content": { "type": "text", "text": text },
        });
        if let Some(id) = message_id {
            value["messageId"] = serde_json::Value::String(id.to_owned());
        }
        SessionUpdate::AgentMessageChunk(
            serde_json::from_value(value).expect("content chunk fixture"),
        )
    }

    /// capability の組み合わせから `AgentCapabilities` を組み立てる。
    fn agent_capabilities(load_session: bool, resume: bool) -> AgentCapabilities {
        // `AgentCapabilities` は non_exhaustive のため default から組み立てる
        let mut capabilities = AgentCapabilities::default();
        capabilities.load_session = load_session;
        if resume {
            capabilities.session_capabilities.resume =
                Some(agent_client_protocol::schema::v1::SessionResumeCapabilities::default());
        }
        capabilities
    }

    fn resume_request(agent_session_id: Option<&str>, allow_fresh: bool) -> ResumeRequest {
        ResumeRequest {
            agent_session_id: agent_session_id.map(str::to_owned),
            allow_fresh,
        }
    }

    fn elicitation_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "strategy": { "type": "string", "enum": ["conservative", "balanced"] },
                "count": { "type": "integer" },
                "confirm": { "type": "boolean" },
                "tags": { "type": "array" }
            },
            "required": ["strategy"]
        })
    }

    #[test]
    fn elicitation_content_validation_accepts_valid_values() {
        let schema = elicitation_schema();
        assert!(
            validate_elicitation_content(
                &schema,
                &serde_json::json!({
                    "strategy": "balanced",
                    "count": 3,
                    "confirm": true,
                    "tags": ["a", "b"]
                })
            )
            .is_ok()
        );
        // properties を持たない schema は検証しない
        assert!(
            validate_elicitation_content(
                &serde_json::json!({ "type": "object" }),
                &serde_json::json!({ "anything": 1 })
            )
            .is_ok()
        );
    }

    #[test]
    fn elicitation_content_validation_rejects_invalid_values() {
        let schema = elicitation_schema();
        // 必須欠落
        assert!(
            validate_elicitation_content(&schema, &serde_json::json!({}))
                .expect_err("required")
                .contains("missing required property")
        );
        // enum 外
        assert!(
            validate_elicitation_content(&schema, &serde_json::json!({ "strategy": "fast" }))
                .expect_err("enum")
                .contains("allowed values")
        );
        // 型不一致
        assert!(
            validate_elicitation_content(
                &schema,
                &serde_json::json!({ "strategy": "balanced", "count": "3" })
            )
            .expect_err("integer")
            .contains("must be an integer")
        );
        // 未知プロパティ
        assert!(
            validate_elicitation_content(
                &schema,
                &serde_json::json!({ "strategy": "balanced", "extra": 1 })
            )
            .expect_err("unknown property")
            .contains("unexpected property")
        );
    }

    #[test]
    fn elicitation_content_validation_checks_titled_enum() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "country": {
                    "type": "string",
                    "oneOf": [{ "const": "jp", "title": "Japan" }]
                }
            }
        });
        assert!(
            validate_elicitation_content(&schema, &serde_json::json!({ "country": "jp" })).is_ok()
        );
        assert!(
            validate_elicitation_content(&schema, &serde_json::json!({ "country": "us" }))
                .expect_err("oneOf")
                .contains("allowed options")
        );
    }

    #[test]
    fn plan_resume_prefers_session_resume_over_load() {
        // 両対応の場合は履歴 Replay のない `session/resume` を優先する
        let capabilities = agent_capabilities(true, true);
        let resume = resume_request(Some("ses_1"), true);
        match plan_resume(&capabilities, Some(&resume)).expect("plan") {
            ResumePlan::Resume(id) => assert_eq!(id, "ses_1"),
            ResumePlan::Load(_) | ResumePlan::Fresh => {
                panic!("session/resume must be preferred")
            }
        }
    }

    #[test]
    fn plan_resume_falls_back_to_load_then_fresh() {
        // load のみ対応 → session/load (Replay は呼び出し側で破棄)
        let capabilities = agent_capabilities(true, false);
        let resume = resume_request(Some("ses_1"), true);
        match plan_resume(&capabilities, Some(&resume)).expect("plan") {
            ResumePlan::Load(id) => assert_eq!(id, "ses_1"),
            ResumePlan::Resume(_) | ResumePlan::Fresh => panic!("session/load expected"),
        }

        // どちらも非対応 + allow_fresh → 新規作成 (Replay フォールバック)
        let capabilities = agent_capabilities(false, false);
        assert!(matches!(
            plan_resume(&capabilities, Some(&resume)).expect("plan"),
            ResumePlan::Fresh
        ));

        // どちらも非対応 + allow_fresh なし → ネイティブ復元不可
        let resume = resume_request(Some("ses_1"), false);
        let err = plan_resume(&capabilities, Some(&resume)).expect_err("native-only must fail");
        assert!(err.downcast_ref::<NativeResumeUnavailable>().is_some());
    }

    #[test]
    fn plan_resume_without_ids_is_fresh() {
        let capabilities = agent_capabilities(true, true);
        // 再開指定なし
        assert!(matches!(
            plan_resume(&capabilities, None).expect("plan"),
            ResumePlan::Fresh
        ));
        // エージェント側IDが未記録 + allow_fresh (履歴 Replay 前提の新規作成)
        let resume = resume_request(None, true);
        assert!(matches!(
            plan_resume(&capabilities, Some(&resume)).expect("plan"),
            ResumePlan::Fresh
        ));
        // エージェント側IDが未記録 + allow_fresh なし (自動レジューム) は
        // ネイティブ復元不可 (履歴 Replay での明示再開が必要)
        let resume = resume_request(None, false);
        let err = plan_resume(&capabilities, Some(&resume)).expect_err("native-only must fail");
        assert!(err.downcast_ref::<NativeResumeUnavailable>().is_some());
    }

    #[test]
    fn accumulator_emits_completed_message_on_id_switch() {
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();
        let events = map_session_update(text_chunk("こん", Some("m1")), &mut acc, &mut caps);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], DriverEvent::Delta(_)));

        // 同一ID: 差分のみ
        let events = map_session_update(text_chunk("にちは", Some("m1")), &mut acc, &mut caps);
        assert_eq!(events.len(), 1);

        // 別ID: 直前メッセージの完成イベント + 新差分
        let events = map_session_update(text_chunk("次", Some("m2")), &mut acc, &mut caps);
        assert_eq!(events.len(), 2);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                message_id,
                text,
                is_complete,
            }) => {
                assert_eq!(message_id, "m1");
                assert_eq!(text, "こんにちは");
                assert!(is_complete);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        // ターン終了で残りをフラッシュ
        let finished = acc.finish_all();
        assert_eq!(finished.len(), 1);
        match &finished[0] {
            UnifiedEventPayload::AgentMessage {
                message_id, text, ..
            } => {
                assert_eq!(message_id, "m2");
                assert_eq!(text, "次");
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn message_chunks_without_id_accumulate_into_single_message() {
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();
        map_session_update(text_chunk("a", None), &mut acc, &mut caps);
        map_session_update(text_chunk("b", None), &mut acc, &mut caps);
        let finished = acc.finish_all();
        assert_eq!(finished.len(), 1);
        match &finished[0] {
            UnifiedEventPayload::AgentMessage {
                message_id, text, ..
            } => {
                assert_eq!(message_id, "acp-message");
                assert_eq!(text, "ab");
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn maps_tool_call_with_diff_content() {
        let tool_call = ToolCall::new("t-1", "Edit file")
            .kind(ToolKind::Edit)
            .status(ToolCallStatus::Completed)
            .content(vec![ToolCallContent::Diff(
                agent_client_protocol::schema::v1::Diff::new("src/lib.rs", "old\nnew\n"),
            )])
            .locations(vec![
                agent_client_protocol::schema::v1::ToolCallLocation::new("src/lib.rs"),
            ]);
        let payload = tool_call_event(&tool_call);
        match payload {
            UnifiedEventPayload::ToolCall {
                tool_call_id,
                kind,
                status,
                diff,
                locations,
                ..
            } => {
                assert_eq!(tool_call_id, "t-1");
                assert_eq!(kind, "edit");
                assert_eq!(status, "completed");
                assert_eq!(locations, vec!["src/lib.rs"]);
                let diff = diff.expect("diff");
                assert_eq!(diff.path, "src/lib.rs");
                assert!(diff.additions >= 1);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn maps_plan_update() {
        let plan = Plan::new(vec![AcpPlanEntry::new(
            "Run tests",
            PlanEntryPriority::High,
            agent_client_protocol::schema::v1::PlanEntryStatus::InProgress,
        )]);
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();
        let events = map_session_update(SessionUpdate::Plan(plan), &mut acc, &mut caps);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::PlanUpdate { entries }) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].title, "Run tests");
                assert_eq!(entries[0].status, "in_progress");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn builds_unified_diff_with_counts() {
        let diff = build_file_diff("a.txt", "one\ntwo\n", "one\ntwo\nthree\n");
        assert_eq!(diff.additions, 1);
        assert_eq!(diff.deletions, 0);
        assert!(diff.unified_diff.contains("+three"));
        assert_eq!(diff.old_text.as_deref(), Some("one\ntwo\n"));
        assert_eq!(diff.new_text.as_deref(), Some("one\ntwo\nthree\n"));
    }

    #[test]
    fn resolves_relative_paths_against_cwd() {
        let cwd = PathBuf::from("/tmp/session");
        assert_eq!(
            resolve_path(&cwd, Path::new("src/lib.rs")),
            PathBuf::from("/tmp/session/src/lib.rs")
        );
        assert_eq!(
            resolve_path(&cwd, Path::new("/etc/hosts")),
            PathBuf::from("/etc/hosts")
        );
    }

    #[test]
    fn base64_encodes_terminal_output() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn maps_session_info_update() {
        use agent_client_protocol::schema::v1::SessionInfoUpdate;

        let update = SessionInfoUpdate::new().title("Refactor UI layout".to_owned());
        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState::default();
        let events = map_session_update(
            SessionUpdate::SessionInfoUpdate(update),
            &mut acc,
            &mut caps,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::SessionTitleChanged { title }) => {
                assert_eq!(title, "Refactor UI layout");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn capabilities_partial_updates_preserve_existing_state() {
        use agent_client_protocol::schema::v1::{
            AvailableCommand, AvailableCommandsUpdate, CurrentModeUpdate,
        };

        let mut acc = StreamingAccumulator::default();
        let mut caps = SessionCapabilitiesState {
            current_mode: Some("ask".to_owned()),
            available_modes: vec![ModeInfo {
                mode_id: "ask".to_owned(),
                name: "Ask".to_owned(),
                description: None,
            }],
            available_commands: Vec::new(),
            config_options: Vec::new(),
        };

        // コマンド一覧の更新が届いたとき、既存の current_mode / available_modes が保持されること
        let cmd_update =
            AvailableCommandsUpdate::new(vec![AvailableCommand::new("review", "コードレビュー")]);
        let events = map_session_update(
            SessionUpdate::AvailableCommandsUpdate(cmd_update),
            &mut acc,
            &mut caps,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                current_mode,
                available_modes,
                available_commands,
                ..
            }) => {
                assert_eq!(current_mode.as_deref(), Some("ask"));
                assert_eq!(available_modes.len(), 1);
                assert_eq!(available_commands.len(), 1);
                assert_eq!(available_commands[0].name, "review");
            }
            other => panic!("unexpected event: {other:?}"),
        }

        // モード変更が届いたとき、直前に追加された available_commands が保持されること
        let mode_update = CurrentModeUpdate::new("code");
        let events = map_session_update(
            SessionUpdate::CurrentModeUpdate(mode_update),
            &mut acc,
            &mut caps,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                current_mode,
                available_modes,
                available_commands,
                ..
            }) => {
                assert_eq!(current_mode.as_deref(), Some("code"));
                assert_eq!(available_modes.len(), 1);
                assert_eq!(available_commands.len(), 1);
                assert_eq!(available_commands[0].name, "review");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn maps_grouped_select_options() {
        use agent_client_protocol::schema::v1::{
            SessionConfigOption, SessionConfigSelect, SessionConfigSelectGroup,
            SessionConfigSelectOption,
        };

        let option = SessionConfigOption::new(
            "model",
            "Model",
            SessionConfigKind::Select(SessionConfigSelect::new(
                "gpt-4",
                vec![
                    SessionConfigSelectGroup::new(
                        "openai",
                        "OpenAI",
                        vec![
                            SessionConfigSelectOption::new("gpt-4", "GPT-4"),
                            SessionConfigSelectOption::new("gpt-3.5", "GPT-3.5"),
                        ],
                    ),
                    SessionConfigSelectGroup::new(
                        "anthropic",
                        "Anthropic",
                        vec![SessionConfigSelectOption::new("claude-3", "Claude 3")],
                    ),
                ],
            )),
        );

        let mapped = map_config_option(&option);
        assert_eq!(mapped.key, "model");
        assert_eq!(mapped.name, "Model");
        assert_eq!(
            mapped.current_value,
            serde_json::Value::String("gpt-4".to_owned())
        );
        assert_eq!(
            mapped.options,
            vec![
                serde_json::Value::String("gpt-4".to_owned()),
                serde_json::Value::String("gpt-3.5".to_owned()),
                serde_json::Value::String("claude-3".to_owned()),
            ]
        );
    }
}
