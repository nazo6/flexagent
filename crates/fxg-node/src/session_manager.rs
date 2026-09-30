//! セッション管理 (エージェント起動・イベント配信・プロンプトキュー)。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §1〜§4、
//! `docs/01-architecture-and-sync.md` §2.2〜§2.4。
//!
//! - **ドライバ統合**: [`AgentDriver`] を起動し、[`DriverEvent`] を
//!   [`SessionEventBus`] へ流す (永続化対象は `record`、差分は `publish_delta`)。
//! - **ターン開始スナップショット**: `SendPrompt` 直前に Shadow Git Tree で
//!   Tree Hash を取得し、`UserMessage.snapshot_tree_hash` として記録する
//!   (Revert の基準点)。
//! - **コマンド冪等性**: `command_id` の重複送信を検出し
//!   [`NodeError::CommandDuplicate`] を返す (ダブルタップ・WS 再送対策)。
//! - **Pending Queue**: 実行中 (busy) の `SendPrompt` はキューに積み、
//!   ターン終了 (`status = idle`) 時に自動で次を送信する。
//! - **承認解決の冪等化**: 2 回目以降の応答は
//!   [`NodeError::AlreadyResolved`] を返す。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fxg_acp::registry::{AcpRegistry, RegistryIndex};
use fxg_acp::{AcpDriver, ActiveSessionHandle, AgentDriver, AgentLaunchSpec, DriverEvent};
use fxg_protocol::common::{PermissionOption, SessionControlAction, SessionStatus};
use fxg_protocol::events::UnifiedEventPayload;
use fxg_protocol::ipc::AttachMode;
use fxg_protocol::util::uuid_v7;
use tokio::sync::mpsc;

use crate::error::NodeError;
use crate::paths::NodePaths;
use crate::project;
use crate::session::SessionEventBus;
use crate::snapshot::ShadowGitTree;

/// 処理済み `command_id` を保持する上限 (冪等性判定の履歴)。
const PROCESSED_COMMAND_HISTORY: usize = 256;

/// エージェント起動スペックからドライバを選択するファクトリ。
pub type DriverFactory =
    Arc<dyn Fn(&AgentLaunchSpec) -> anyhow::Result<Box<dyn AgentDriver>> + Send + Sync>;

/// 既定のドライバファクトリ。
///
/// `opencode2` (ブリッジモード) は Phase 3 の残タスクのため、現時点では
/// `agents.opencode_mode = "acp"` を案内するエラーを返す。
pub fn default_driver_factory() -> DriverFactory {
    Arc::new(|spec: &AgentLaunchSpec| match spec.driver_kind.as_str() {
        "acp" => Ok(Box::new(AcpDriver::new()) as Box<dyn AgentDriver>),
        "opencode2" => Err(anyhow::anyhow!(
            "opencode2 bridge driver is not implemented yet \
             (set [agents] opencode_mode = \"acp\" to use ACP mode)"
        )),
        other => Err(anyhow::anyhow!("unknown driver kind: {other}")),
    })
}

/// `ensure_session` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct EnsureSessionOutcome {
    /// 開始したセッションID
    pub session_id: String,
    /// CLI のアタッチモード
    pub attach_mode: AttachMode,
}

/// セッション一覧の 1 行 (IPC 用)。
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSessionSummary {
    /// セッションID
    pub session_id: String,
    /// エージェントID
    pub agent_id: String,
    /// 作業ディレクトリ
    pub cwd: PathBuf,
    /// 現在の状態
    pub status: SessionStatus,
    /// ターン実行中 (busy) か
    pub busy: bool,
    /// 待機中のプロンプト件数
    pub queued_prompts: usize,
}

/// `fxg inbox` 用の承認待ちエントリ。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingPermissionSummary {
    /// 対象セッションID
    pub session_id: String,
    /// ACP request id
    pub request_id: String,
    /// ツール名
    pub tool_name: String,
    /// 要約
    pub summary: String,
    /// 選択肢
    pub options: Vec<PermissionOption>,
}

/// セッション管理 (デーモンが 1 つ保持する)。
#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Inner>,
}

struct Inner {
    bus: SessionEventBus,
    paths: NodePaths,
    node_id: String,
    registry: AcpRegistry,
    factory: DriverFactory,
    sessions: Mutex<Sessions>,
}

#[derive(Default)]
struct Sessions {
    active: HashMap<String, ActiveSession>,
    /// 直近に処理した `command_id` (古いものから破棄)
    processed_commands: VecDeque<String>,
}

struct ActiveSession {
    /// 操作ハンドル (ガードを跨いで await しないよう Arc で共有)
    handle: Arc<dyn ActiveSessionHandle>,
    cwd: PathBuf,
    agent_id: String,
    status: SessionStatus,
    busy: bool,
    pending_prompts: VecDeque<PendingPrompt>,
    /// 承認待ち (`request_id` → 表示用情報)
    pending_permissions: HashMap<String, PendingPermissionSummary>,
}

#[derive(Debug, Clone)]
struct PendingPrompt {
    text: String,
    client_source: String,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        f.debug_struct("SessionManager")
            .field("active_sessions", &sessions.active.len())
            .finish()
    }
}

impl SessionManager {
    /// マネージャを作成する。
    pub fn new(
        bus: SessionEventBus,
        paths: NodePaths,
        node_id: impl Into<String>,
        registry: AcpRegistry,
    ) -> Self {
        Self::with_factory(bus, paths, node_id, registry, default_driver_factory())
    }

    /// ドライバファクトリを差し替えて作成する (テスト用)。
    pub fn with_factory(
        bus: SessionEventBus,
        paths: NodePaths,
        node_id: impl Into<String>,
        registry: AcpRegistry,
        factory: DriverFactory,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                bus,
                paths,
                node_id: node_id.into(),
                registry,
                factory,
                sessions: Mutex::new(Sessions::default()),
            }),
        }
    }

    /// レジストリ (CLI の `fxg agents` 等から利用)。
    pub fn registry(&self) -> &AcpRegistry {
        &self.inner.registry
    }

    /// セッションイベントバス。
    pub fn bus(&self) -> &SessionEventBus {
        &self.inner.bus
    }

    /// `command_id` を処理済みとして記録する。
    ///
    /// 既に処理済みの場合は `false` を返す (呼び出し側で
    /// [`NodeError::CommandDuplicate`] を返す)。
    fn begin_command(&self, command_id: &str) -> bool {
        let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
        if sessions
            .processed_commands
            .iter()
            .any(|id| id == command_id)
        {
            return false;
        }
        sessions.processed_commands.push_back(command_id.to_owned());
        while sessions.processed_commands.len() > PROCESSED_COMMAND_HISTORY {
            sessions.processed_commands.pop_front();
        }
        true
    }

    /// 新規セッションを開始する (`fxg run`)。
    pub async fn ensure_session(
        &self,
        command_id: &str,
        cwd: &Path,
        agent_id: &str,
        extra_args: &[String],
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let resolved = project::resolve_project(cwd, &self.inner.node_id).await?;
        let session_id = uuid_v7();

        // 起動スペック解決 (カスタム/ビルトインはインデックス不要のため先に試す)
        let spec = match self
            .inner
            .registry
            .launch_spec(agent_id, &RegistryIndex::default(), extra_args)
            .await
        {
            Ok(spec) => spec,
            Err(_) => {
                let index = self
                    .inner
                    .registry
                    .index(false)
                    .await
                    .map_err(|err| NodeError::Agent(err.to_string()))?;
                self.inner
                    .registry
                    .launch_spec(agent_id, &index, extra_args)
                    .await
                    .map_err(|err| NodeError::Agent(err.to_string()))?
            }
        };

        // SessionCreated (node_seq = 1)
        let (git_branch, is_worktree) = branch_and_worktree(&resolved.local_path).await;
        self.inner
            .bus
            .create_session(
                &session_id,
                UnifiedEventPayload::SessionCreated {
                    node_id: self.inner.node_id.clone(),
                    project_id: resolved.project_id.clone(),
                    project_name: resolved.name.clone(),
                    local_path: resolved.local_path.to_string_lossy().into_owned(),
                    git_branch,
                    is_worktree,
                    agent_id: spec.agent_id.clone(),
                    parent_session_id: None,
                    fork_from_node_seq: None,
                    title: format!("{} @ {}", spec.display_name, resolved.name),
                },
            )
            .await?;

        // ドライバ起動 (失敗した場合はセッションを error 状態で記録する)
        let driver =
            (self.inner.factory)(&spec).map_err(|err| NodeError::Agent(err.to_string()))?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let handle = match driver
            .start_session(
                fxg_acp::StartSessionRequest {
                    session_id: session_id.clone(),
                    cwd: resolved.local_path.clone(),
                    launch: spec.clone(),
                    extra_args: extra_args.to_vec(),
                    initial_mode: None,
                },
                event_tx,
            )
            .await
        {
            Ok(handle) => handle,
            Err(err) => {
                let message = format!("failed to start agent: {err:#}");
                tracing::warn!(session_id = %session_id, "{message}");
                self.inner
                    .bus
                    .record(
                        &session_id,
                        UnifiedEventPayload::StatusChanged {
                            status: SessionStatus::Error,
                            error_message: Some(message.clone()),
                        },
                    )
                    .await?;
                return Err(NodeError::Agent(message));
            }
        };

        let handle: Arc<dyn ActiveSessionHandle> = Arc::from(handle);
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions.active.insert(
                session_id.clone(),
                ActiveSession {
                    handle,
                    cwd: resolved.local_path.clone(),
                    agent_id: spec.agent_id.clone(),
                    status: SessionStatus::Idle,
                    busy: false,
                    pending_prompts: VecDeque::new(),
                    pending_permissions: HashMap::new(),
                },
            );
        }

        // イベントポンプ
        let inner = Arc::clone(&self.inner);
        let pump_session_id = session_id.clone();
        tokio::spawn(async move {
            pump_events(inner, pump_session_id, event_rx).await;
        });

        Ok(EnsureSessionOutcome {
            session_id,
            // OpenCode2 純正TUI Attach (ブリッジモード) は Phase 3 の残タスク
            attach_mode: AttachMode::AcpTui,
        })
    }

    /// プロンプトを送信する (busy の場合は Pending Queue へ)。
    pub async fn send_prompt(
        &self,
        command_id: &str,
        session_id: &str,
        text: &str,
        client_source: &str,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        // busy チェックと予約 (check-and-set)
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions
                .active
                .get_mut(session_id)
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
            if session.busy {
                session.pending_prompts.push_back(PendingPrompt {
                    text: text.to_owned(),
                    client_source: client_source.to_owned(),
                });
                return Ok(());
            }
            session.busy = true;
        }

        match self.dispatch_prompt(session_id, text, client_source).await {
            Ok(()) => Ok(()),
            Err(err) => {
                // 送信失敗時は busy を解除して整合性を保つ
                if let Some(session) = self
                    .inner
                    .sessions
                    .lock()
                    .expect("sessions poisoned")
                    .active
                    .get_mut(session_id)
                {
                    session.busy = false;
                }
                Err(err)
            }
        }
    }

    /// 1 ターンを開始する (スナップショット → `UserMessage` 記録 → ドライバ送信)。
    async fn dispatch_prompt(
        &self,
        session_id: &str,
        text: &str,
        client_source: &str,
    ) -> Result<(), NodeError> {
        let cwd = self.session_cwd(session_id)?;
        let snapshot_tree_hash = snapshot_tree_hash(
            &cwd,
            &self.inner.paths.snapshot_index_path(session_id),
            session_id,
        )
        .await;

        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::UserMessage {
                    text: text.to_owned(),
                    attachments: Vec::new(),
                    client_source: client_source.to_owned(),
                    snapshot_tree_hash,
                },
            )
            .await?;

        let handle = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions
                .active
                .get(session_id)
                .map(|session| Arc::clone(&session.handle))
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?
        };
        handle
            .send_prompt(text.to_owned())
            .await
            .map_err(|err| NodeError::Server(format!("failed to send prompt: {err:#}")))
    }

    /// 承認リクエストへ応答する (2 回目以降は `AlreadyResolved`)。
    pub async fn respond_permission(
        &self,
        command_id: &str,
        session_id: &str,
        request_id: &str,
        selected_option_id: &str,
        resolved_by: &str,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let handle = {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions
                .active
                .get_mut(session_id)
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
            if session.pending_permissions.remove(request_id).is_none() {
                return Err(NodeError::AlreadyResolved(request_id.to_owned()));
            }
            Arc::clone(&session.handle)
        };
        handle
            .respond_permission(request_id.to_owned(), selected_option_id.to_owned())
            .await
            .map_err(|err| NodeError::Server(format!("failed to respond: {err:#}")))?;

        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::PermissionResolved {
                    request_id: request_id.to_owned(),
                    selected_option_id: selected_option_id.to_owned(),
                    resolved_by: resolved_by.to_owned(),
                },
            )
            .await?;
        Ok(())
    }

    /// モード切替 / 設定変更 / キャンセル / Kill。
    pub async fn control(
        &self,
        command_id: &str,
        session_id: &str,
        action: &SessionControlAction,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        // ロックは await を跨がない (ハンドルを複製してから操作する)
        let handle = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions
                .active
                .get(session_id)
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
            Arc::clone(&session.handle)
        };
        match action {
            SessionControlAction::SetMode { mode_id } => handle
                .set_mode(mode_id.clone())
                .await
                .map_err(|err| NodeError::Server(format!("failed to set mode: {err:#}"))),
            SessionControlAction::SetConfig { key, value } => handle
                .set_config(key.clone(), value.clone())
                .await
                .map_err(|err| NodeError::Server(format!("failed to set config: {err:#}"))),
            SessionControlAction::Cancel => handle
                .cancel_turn()
                .await
                .map_err(|err| NodeError::Server(format!("failed to cancel: {err:#}"))),
            SessionControlAction::Kill => handle
                .shutdown()
                .await
                .map_err(|err| NodeError::Server(format!("failed to shutdown: {err:#}"))),
        }
    }

    /// 稼働中セッション一覧。
    pub fn list_active(&self) -> Vec<ActiveSessionSummary> {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        let mut list: Vec<ActiveSessionSummary> = sessions
            .active
            .iter()
            .map(|(session_id, session)| ActiveSessionSummary {
                session_id: session_id.clone(),
                agent_id: session.agent_id.clone(),
                cwd: session.cwd.clone(),
                status: session.status,
                busy: session.busy,
                queued_prompts: session.pending_prompts.len(),
            })
            .collect();
        list.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        list
    }

    /// 承認待ちリクエスト一覧 (`fxg inbox list`)。
    pub fn pending_permissions(&self) -> Vec<PendingPermissionSummary> {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        let mut list: Vec<PendingPermissionSummary> = sessions
            .active
            .values()
            .flat_map(|session| session.pending_permissions.values().cloned())
            .collect();
        list.sort_by(|a, b| a.request_id.cmp(&b.request_id));
        list
    }

    /// 全セッションのエージェントプロセスを終了する (`fxg kill-all`)。
    pub async fn shutdown_all(&self) -> Vec<String> {
        let handles: Vec<(String, Arc<dyn ActiveSessionHandle>)> = {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions
                .active
                .drain()
                .map(|(session_id, session)| (session_id, session.handle))
                .collect()
        };
        let mut stopped = Vec::with_capacity(handles.len());
        for (session_id, handle) in handles {
            if let Err(err) = handle.shutdown().await {
                tracing::warn!(session_id = %session_id, "failed to shutdown: {err:#}");
            }
            stopped.push(session_id);
        }
        stopped
    }

    fn session_cwd(&self, session_id: &str) -> Result<PathBuf, NodeError> {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        sessions
            .active
            .get(session_id)
            .map(|session| session.cwd.clone())
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))
    }
}

/// ドライバイベントを永続化・配信し、ステータス遷移とキューを管理する。
async fn pump_events(
    inner: Arc<Inner>,
    session_id: String,
    mut event_rx: mpsc::UnboundedReceiver<DriverEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        match event {
            DriverEvent::Delta(delta) => inner.bus.publish_delta(&session_id, delta),
            DriverEvent::Failed { message } => {
                mark_status(
                    &inner,
                    &session_id,
                    SessionStatus::Error,
                    Some(message.clone()),
                );
                let _ = inner
                    .bus
                    .record(
                        &session_id,
                        UnifiedEventPayload::StatusChanged {
                            status: SessionStatus::Error,
                            error_message: Some(message),
                        },
                    )
                    .await;
            }
            DriverEvent::Event(payload) => {
                match &payload {
                    UnifiedEventPayload::StatusChanged {
                        status,
                        error_message,
                    } => {
                        mark_status(&inner, &session_id, *status, error_message.clone());
                    }
                    UnifiedEventPayload::PermissionRequest {
                        request_id,
                        tool_name,
                        summary,
                        options,
                        ..
                    } => {
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        if let Some(session) = sessions.active.get_mut(&session_id) {
                            session.pending_permissions.insert(
                                request_id.clone(),
                                PendingPermissionSummary {
                                    session_id: session_id.clone(),
                                    request_id: request_id.clone(),
                                    tool_name: tool_name.clone(),
                                    summary: summary.clone(),
                                    options: options.clone(),
                                },
                            );
                        }
                    }
                    _ => {}
                }
                if inner.bus.record(&session_id, payload).await.is_ok() {
                    maybe_dispatch_queued(&inner, &session_id).await;
                }
            }
        }
    }
    // チャネルが閉じた = ドライバ終了
    mark_status(&inner, &session_id, SessionStatus::Stopped, None);
    let mut sessions = inner.sessions.lock().expect("sessions poisoned");
    sessions.active.remove(&session_id);
}

/// セッションステータスを更新する (busy も同期)。
fn mark_status(
    inner: &Inner,
    session_id: &str,
    status: SessionStatus,
    _error_message: Option<String>,
) {
    let mut sessions = inner.sessions.lock().expect("sessions poisoned");
    if let Some(session) = sessions.active.get_mut(session_id) {
        session.busy = matches!(
            status,
            SessionStatus::Running | SessionStatus::WaitingPermission
        );
        session.status = status;
    }
}

/// ターン終了後にキュー先頭のプロンプトを送信する。
async fn maybe_dispatch_queued(inner: &Arc<Inner>, session_id: &str) {
    let next = {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        let Some(session) = sessions.active.get_mut(session_id) else {
            return;
        };
        if session.busy {
            return;
        }
        match session.pending_prompts.pop_front() {
            Some(prompt) => {
                session.busy = true;
                Some(prompt)
            }
            None => None,
        }
    };
    let Some(prompt) = next else {
        return;
    };

    let cwd = {
        let sessions = inner.sessions.lock().expect("sessions poisoned");
        sessions
            .active
            .get(session_id)
            .map(|session| session.cwd.clone())
    };
    let Some(cwd) = cwd else { return };

    let snapshot_tree_hash = snapshot_tree_hash(
        &cwd,
        &inner.paths.snapshot_index_path(session_id),
        session_id,
    )
    .await;
    if inner
        .bus
        .record(
            session_id,
            UnifiedEventPayload::UserMessage {
                text: prompt.text.clone(),
                attachments: Vec::new(),
                client_source: prompt.client_source.clone(),
                snapshot_tree_hash,
            },
        )
        .await
        .is_err()
    {
        let _ = inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Error,
                    error_message: Some("failed to record queued prompt".to_owned()),
                },
            )
            .await;
        return;
    }

    let handle = {
        let sessions = inner.sessions.lock().expect("sessions poisoned");
        let Some(session) = sessions.active.get(session_id) else {
            return;
        };
        Arc::clone(&session.handle)
    };
    if let Err(err) = handle.send_prompt(prompt.text).await {
        tracing::warn!(session_id = %session_id, "failed to dispatch queued prompt: {err:#}");
    }
}

/// 作業ディレクトリの Shadow Git Tree スナップショットを取得する。
///
/// 非 Git ディレクトリや失敗時は `None` (Revert 不可のターンとして記録)。
async fn snapshot_tree_hash(cwd: &Path, index_path: &Path, session_id: &str) -> Option<String> {
    let tree = ShadowGitTree::new(cwd, index_path, session_id);
    match tree.snapshot().await {
        Ok(outcome) => outcome.tree_hash().map(str::to_owned),
        Err(err) => {
            tracing::debug!(session_id, "snapshot skipped: {err}");
            None
        }
    }
}

/// 現在のブランチと Worktree 判定を返す。
async fn branch_and_worktree(cwd: &Path) -> (Option<String>, bool) {
    let branch = crate::git::try_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()
        .flatten();
    let git_dir = crate::git::try_git(cwd, &["rev-parse", "--git-dir"])
        .await
        .ok()
        .flatten();
    let is_worktree = git_dir
        .as_deref()
        .map(|dir| dir.contains("worktrees"))
        .unwrap_or(false);
    (branch, is_worktree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockAgent;
    use fxg_db::{Db, DbRole};
    use fxg_protocol::config::{AgentsConfig, CustomAgentConfig};

    async fn setup() -> (SessionManager, MockAgent, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory(DbRole::Node).await.expect("db");
        db.upsert_node(&fxg_db::NodeRecord::new(
            "test-node",
            "Test Node",
            "linux",
            "aarch64",
            "0.1.0",
        ))
        .await
        .expect("node");
        let bus = SessionEventBus::new(db);
        let paths = NodePaths::new(dir.path().to_path_buf());
        paths.ensure_dirs().expect("dirs");

        // カスタムエージェント "mock" を登録 (インデックス不要で解決される)
        let mut custom = std::collections::BTreeMap::new();
        custom.insert(
            "mock".to_owned(),
            CustomAgentConfig {
                name: Some("Mock Agent".to_owned()),
                command: "mock".to_owned(),
                args: Vec::new(),
                env: Default::default(),
            },
        );
        let config = AgentsConfig {
            custom,
            ..AgentsConfig::default()
        };
        let registry = AcpRegistry::new(dir.path(), &config);

        let mock = MockAgent::default();
        let manager =
            SessionManager::with_factory(bus, paths, "test-node", registry, mock.factory());
        (manager, mock, dir)
    }

    /// セッションが Idle になるまで待つ (ドライバの初回イベント処理を待つ)。
    async fn wait_idle(manager: &SessionManager, session_id: &str) {
        for _ in 0..100 {
            if manager
                .list_active()
                .iter()
                .any(|session| session.session_id == session_id && !session.busy)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("session did not become idle");
    }

    #[tokio::test]
    async fn ensure_session_records_created_event_and_start() {
        let (manager, _mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[])
            .await
            .expect("ensure");
        assert!(!outcome.session_id.is_empty());
        assert_eq!(outcome.attach_mode, AttachMode::AcpTui);

        // SessionCreated が永続化され、sessions 投影が存在する
        let session = manager
            .bus()
            .db()
            .get_session(&outcome.session_id)
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(session.agent_id, "mock");
    }

    #[tokio::test]
    async fn send_prompt_records_user_message_and_dispatches() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[])
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        manager
            .send_prompt("c2", &outcome.session_id, "hello", "cli")
            .await
            .expect("prompt");

        // ドライバへ送信され、busy になる
        for _ in 0..100 {
            if !mock.prompts().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(mock.prompts(), vec!["hello"]);
        let summary = manager
            .list_active()
            .into_iter()
            .find(|session| session.session_id == outcome.session_id)
            .expect("active");
        assert!(summary.busy);
    }

    #[tokio::test]
    async fn busy_prompt_is_queued_and_drained_on_idle() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[])
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        manager
            .send_prompt("c2", &outcome.session_id, "first", "cli")
            .await
            .expect("first");
        // busy 中は キューへ
        manager
            .send_prompt("c3", &outcome.session_id, "second", "web")
            .await
            .expect("second queued");
        let summary = manager
            .list_active()
            .into_iter()
            .find(|session| session.session_id == outcome.session_id)
            .expect("active");
        assert_eq!(summary.queued_prompts, 1);
        assert_eq!(mock.prompts(), vec!["first"]);

        // ターン完了 (idle) → キューが自動送信される
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        for _ in 0..100 {
            if mock.prompts().len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(mock.prompts(), vec!["first", "second"]);
    }

    #[tokio::test]
    async fn duplicate_command_id_is_rejected() {
        let (manager, _mock, dir) = setup().await;
        manager
            .ensure_session("c1", dir.path(), "mock", &[])
            .await
            .expect("ensure");
        let err = manager
            .ensure_session("c1", dir.path(), "mock", &[])
            .await
            .expect_err("duplicate");
        assert!(matches!(err, NodeError::CommandDuplicate(_)));
    }

    #[tokio::test]
    async fn permission_resolution_is_idempotent() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[])
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        // ドライバから承認リクエストを注入
        mock.emit(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
            request_id: "req-1".to_owned(),
            tool_name: "terminal".to_owned(),
            summary: "Run tests".to_owned(),
            options: vec![PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow once".to_owned(),
                kind: "allow_once".to_owned(),
            }],
            details: serde_json::json!({}),
        }));
        for _ in 0..100 {
            if !manager.pending_permissions().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(manager.pending_permissions().len(), 1);

        manager
            .respond_permission("c2", &outcome.session_id, "req-1", "allow_once", "cli")
            .await
            .expect("resolve");
        assert!(manager.pending_permissions().is_empty());

        // 2 回目は ALREADY_RESOLVED
        let err = manager
            .respond_permission("c3", &outcome.session_id, "req-1", "allow_once", "cli")
            .await
            .expect_err("already resolved");
        assert!(matches!(err, NodeError::AlreadyResolved(_)));
    }

    #[tokio::test]
    async fn session_status_transitions_on_driver_events() {
        let (manager, mock, dir) = setup().await;
        manager
            .ensure_session("c1", dir.path(), "mock", &[])
            .await
            .expect("ensure");
        assert_eq!(manager.list_active().len(), 1);

        // ドライバからの StatusChanged がセッション状態へ反映される
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Stopped,
            error_message: None,
        }));
        for _ in 0..100 {
            let active = manager.list_active();
            if active
                .iter()
                .any(|session| session.status == SessionStatus::Stopped)
            {
                // ステータスは node.db の sessions 投影にも反映される
                let session_id = active[0].session_id.clone();
                let row = manager
                    .bus()
                    .db()
                    .get_session(&session_id)
                    .await
                    .expect("query")
                    .expect("session row");
                assert_eq!(row.status, SessionStatus::Stopped);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("status did not become stopped");
    }
}
