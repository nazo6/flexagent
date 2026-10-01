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

use fxg_acp::registry::{AcpRegistry, OPENCODE2_ID, RegistryIndex};
use fxg_acp::{
    AcpDriver, ActiveSessionHandle, AgentDriver, AgentLaunchSpec, DriverEvent, OpenCode2Driver,
};
use fxg_protocol::common::{PermissionOption, SessionControlAction, SessionStatus};
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
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

/// `session_events` をページ読み込みする際のバッチサイズ。
const EVENT_LOAD_BATCH: u32 = 200;

/// エージェント起動スペックからドライバを選択するファクトリ。
pub type DriverFactory =
    Arc<dyn Fn(&AgentLaunchSpec) -> anyhow::Result<Box<dyn AgentDriver>> + Send + Sync>;

/// 既定のドライバファクトリ。
///
/// - `acp`: 標準ACPエージェント ([`AcpDriver`])
/// - `opencode2`: `opencode2 serve` ブリッジ ([`OpenCode2Driver`])
pub fn default_driver_factory() -> DriverFactory {
    Arc::new(|spec: &AgentLaunchSpec| match spec.driver_kind.as_str() {
        "acp" => Ok(Box::new(AcpDriver::new()) as Box<dyn AgentDriver>),
        "opencode2" => Ok(Box::new(OpenCode2Driver::new()) as Box<dyn AgentDriver>),
        other => Err(anyhow::anyhow!("unknown driver kind: {other}")),
    })
}

/// ドライバの純正TUI Attach 情報から CLI のアタッチモードを決める。
fn attach_mode_of(handle: &dyn ActiveSessionHandle) -> AttachMode {
    match handle.native_attach() {
        Some(info) => AttachMode::NativeOpenCodeAttach {
            server_url: info.server_url,
            session_id: info.session_id,
            env: info.env,
        },
        None => AttachMode::AcpTui,
    }
}

/// `ensure_session` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct EnsureSessionOutcome {
    /// 開始したセッションID
    pub session_id: String,
    /// CLI のアタッチモード
    pub attach_mode: AttachMode,
}

/// 明示セッションIDでの開始要求 ([`SessionManager::start_session`])。
///
/// 中央サーバー (`ServerToNodeMsg::StartSession`) やローカル REST
/// (`POST /api/v1/sessions`) から使用する。セッションIDは呼び出し側
/// (中央サーバー) が採番し、全クライアントで相関できるようにする。
#[derive(Debug, Clone)]
pub struct StartSessionParams<'a> {
    /// 相関ID (重複送信の冪等排除)
    pub command_id: &'a str,
    /// セッションID (呼び出し側採番の UUID v7)
    pub session_id: &'a str,
    /// 実行ディレクトリ (Worktree パス含む)
    pub local_path: &'a Path,
    /// エージェントID
    pub agent_id: &'a str,
    /// 初期プロンプト
    pub initial_prompt: Option<&'a str>,
}

/// `revert` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct RevertOutcome {
    /// Revert 基準にした `UserMessage` の `node_seq`
    pub target_node_seq: u64,
    /// 復元先の Tree Hash
    pub restored_tree_hash: String,
    /// 復元直前を退避したバックアップ Tree Hash
    pub backup_tree_hash: Option<String>,
    /// 復元したファイル数
    pub restored_files: usize,
    /// 削除したファイル数
    pub removed_files: usize,
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
    /// 意味のあるタイトルが設定済みか (プロンプト導出またはドライバ通知)
    has_custom_title: bool,
}

#[derive(Debug, Clone)]
struct PendingPrompt {
    text: String,
    client_source: String,
}

struct StartDriverParams<'a> {
    session_id: &'a str,
    title: &'a str,
    cwd: &'a Path,
    spec: &'a AgentLaunchSpec,
    extra_args: Vec<String>,
    initial_mode: Option<String>,
    has_custom_title: bool,
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
        initial_mode: Option<&str>,
        acp: bool,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let resolved = project::resolve_project(cwd, &self.inner.node_id).await?;
        let session_id = uuid_v7();

        let mut spec = self.resolve_launch_spec(agent_id, extra_args).await?;
        // `--acp`: `opencode2` を標準ACPモード (`opencode2 acp`) で起動する
        if acp && spec.agent_id == OPENCODE2_ID {
            spec.args = vec!["acp".to_owned()];
            spec.driver_kind = "acp".to_owned();
        }

        // SessionCreated (node_seq = 1)
        let (git_branch, is_worktree) = branch_and_worktree(&resolved.local_path).await;
        let title = format!("{} @ {}", spec.display_name, resolved.name);
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
                    title: title.clone(),
                },
            )
            .await?;

        let handle = self
            .start_driver_session(StartDriverParams {
                session_id: &session_id,
                title: &title,
                cwd: &resolved.local_path,
                spec: &spec,
                extra_args: extra_args.to_vec(),
                initial_mode: initial_mode.map(str::to_owned),
                has_custom_title: false,
            })
            .await?;

        Ok(EnsureSessionOutcome {
            session_id,
            attach_mode: attach_mode_of(handle.as_ref()),
        })
    }

    /// 明示セッションIDで新規セッションを開始する (`StartSession` / ローカル REST)。
    ///
    /// [`Self::ensure_session`] との違いは (1) `session_id` を呼び出し側が採番する、
    /// (2) `cwd` ではなく解決済みの `local_path` を受け取る、の2点。
    /// プロジェクト解決 (`project_id`) はノード側の正データで行う。
    pub async fn start_session(
        &self,
        params: StartSessionParams<'_>,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(params.command_id) {
            return Err(NodeError::CommandDuplicate(params.command_id.to_owned()));
        }

        let resolved = project::resolve_project(params.local_path, &self.inner.node_id).await?;
        let spec = self.resolve_launch_spec(params.agent_id, &[]).await?;

        // SessionCreated (node_seq = 1)
        let (git_branch, is_worktree) = branch_and_worktree(&resolved.local_path).await;
        let default_title = format!("{} @ {}", spec.display_name, resolved.name);
        let (title, has_custom_title) = if let Some(prompt) = params.initial_prompt
            && let Some(derived) = derive_title_from_prompt(prompt)
        {
            (derived, true)
        } else {
            (default_title, false)
        };
        self.inner
            .bus
            .create_session(
                params.session_id,
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
                    title: title.clone(),
                },
            )
            .await?;

        let handle = self
            .start_driver_session(StartDriverParams {
                session_id: params.session_id,
                title: &title,
                cwd: &resolved.local_path,
                spec: &spec,
                extra_args: Vec::new(),
                initial_mode: None,
                has_custom_title,
            })
            .await?;

        // 初期プロンプト (リモートからのセッション起動時にそのまま実行する)
        if let Some(prompt) = params.initial_prompt.filter(|text| !text.trim().is_empty()) {
            self.start_turn(params.session_id, prompt, "web").await?;
        }

        Ok(EnsureSessionOutcome {
            session_id: params.session_id.to_owned(),
            attach_mode: attach_mode_of(handle.as_ref()),
        })
    }

    /// 起動スペックを解決する (カスタム/ビルトインはインデックス不要)。
    async fn resolve_launch_spec(
        &self,
        agent_id: &str,
        extra_args: &[String],
    ) -> Result<AgentLaunchSpec, NodeError> {
        match self
            .inner
            .registry
            .launch_spec(agent_id, &RegistryIndex::default(), extra_args)
            .await
        {
            Ok(spec) => Ok(spec),
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
                    .map_err(|err| NodeError::Agent(err.to_string()))
            }
        }
    }

    /// ドライバを起動し、active 一覧への登録とイベントポンプの開始を行う。
    ///
    /// 起動失敗時は `StatusChanged(Error)` を記録する。成功時は CLI の
    /// アタッチモード決定に使うハンドルを返す。
    async fn start_driver_session(
        &self,
        params: StartDriverParams<'_>,
    ) -> Result<Arc<dyn ActiveSessionHandle>, NodeError> {
        let driver =
            (self.inner.factory)(params.spec).map_err(|err| NodeError::Agent(err.to_string()))?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let handle = match driver
            .start_session(
                fxg_acp::StartSessionRequest {
                    session_id: params.session_id.to_owned(),
                    title: Some(params.title.to_owned()),
                    cwd: params.cwd.to_path_buf(),
                    launch: params.spec.clone(),
                    extra_args: params.extra_args,
                    initial_mode: params.initial_mode,
                },
                event_tx,
            )
            .await
        {
            Ok(handle) => handle,
            Err(err) => {
                let message = format!("failed to start agent: {err:#}");
                tracing::warn!(session_id = params.session_id, "{message}");
                self.inner
                    .bus
                    .record(
                        params.session_id,
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
                params.session_id.to_owned(),
                ActiveSession {
                    handle: Arc::clone(&handle),
                    cwd: params.cwd.to_path_buf(),
                    agent_id: params.spec.agent_id.clone(),
                    status: SessionStatus::Idle,
                    busy: false,
                    pending_prompts: VecDeque::new(),
                    pending_permissions: HashMap::new(),
                    has_custom_title: params.has_custom_title,
                },
            );
        }

        // イベントポンプ
        let inner = Arc::clone(&self.inner);
        let pump_session_id = params.session_id.to_owned();
        tokio::spawn(async move {
            pump_events(inner, pump_session_id, event_rx).await;
        });
        Ok(handle)
    }

    /// セッションの永続イベントを読み込む (`up_to_node_seq` 以下に限定。`None` は全件)。
    async fn load_session_events(
        &self,
        session_id: &str,
        up_to_node_seq: Option<u64>,
    ) -> Result<Vec<SessionEventEnvelope>, NodeError> {
        let mut cursor = 0u64;
        let mut events = Vec::new();
        loop {
            let batch = self
                .inner
                .bus
                .db()
                .session_events_after(session_id, cursor, EVENT_LOAD_BATCH)
                .await?;
            let full_batch = batch.events.len() as u32 >= EVENT_LOAD_BATCH;
            cursor = batch.cursor;
            for event in batch.events {
                if let Some(limit) = up_to_node_seq
                    && event.node_seq > limit
                {
                    return Ok(events);
                }
                events.push(event);
            }
            if !full_batch {
                return Ok(events);
            }
        }
    }

    /// 指定ターン時点へワークスペースを復元する (`fxg session revert`)。
    ///
    /// - `target_node_seq` は `UserMessage` の `node_seq`
    ///   (省略時は直近のターン)。その時点の `snapshot_tree_hash` へ
    ///   ファイルを復元し、`SessionReverted` を追記する。
    /// - 復元直前の状態はバックアップ Tree として退避される
    ///   (もう一度 Revert すれば元に戻せる)。
    /// - 実行中 (busy) のセッションは [`NodeError::Busy`] で拒否する。
    pub async fn revert(
        &self,
        command_id: &str,
        session_id: &str,
        target_node_seq: Option<u64>,
    ) -> Result<RevertOutcome, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let (cwd, handle) = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions
                .active
                .get(session_id)
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
            if session.busy {
                return Err(NodeError::Busy(session_id.to_owned()));
            }
            (session.cwd.clone(), Arc::clone(&session.handle))
        };

        let events = self
            .load_session_events(session_id, target_node_seq)
            .await?;
        let (target_seq, tree_hash) = find_revert_snapshot(&events).ok_or_else(|| {
            NodeError::InvalidSession(format!(
                "no snapshot found at or before the specified point: {session_id}"
            ))
        })?;

        let tree = ShadowGitTree::new(
            &cwd,
            self.inner.paths.snapshot_index_path(session_id),
            session_id,
        );
        let outcome = tree.restore(&tree_hash).await?;

        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::SessionReverted {
                    target_node_seq: target_seq,
                    restored_tree_hash: tree_hash.clone(),
                    backup_tree_hash: outcome.backup_tree_hash.clone(),
                    restored_files: outcome.restored_files as u64,
                    removed_files: outcome.removed_files as u64,
                },
            )
            .await?;

        // エージェント側の会話巻き戻し (ネイティブ API 対応ドライバのみ)。
        // `target_seq` 以前のユーザーメッセージ数 = 先頭から残すターン数。
        // 標準ACPは未対応のため、失敗は情報ログに留める (ファイル復元は完了している)。
        let keep_turns = events
            .iter()
            .filter(|event| matches!(event.payload, UnifiedEventPayload::UserMessage { .. }))
            .count() as u64;
        if let Err(err) = handle.revert_context(keep_turns).await {
            tracing::info!(
                session_id,
                "agent-side revert is not supported by this driver: {err:#}"
            );
        }

        Ok(RevertOutcome {
            restored_tree_hash: tree_hash,
            backup_tree_hash: outcome.backup_tree_hash,
            restored_files: outcome.restored_files,
            removed_files: outcome.removed_files,
            target_node_seq: target_seq,
        })
    }

    /// 指定ターンから新しいセッションへ分岐する (`fxg session fork`)。
    ///
    /// - 分岐元の `SessionCreated` から `from_node_seq` までの履歴を引き継ぎ、
    ///   新しい `SessionCreated` に `parent_session_id` / `fork_from_node_seq` を
    ///   記録する。
    /// - 標準ACPにはネイティブ Fork API が無いため、`node_seq` までの会話履歴を
    ///   初期コンテキスト (1プロンプト) として注入する
    ///   (`client_source = "fork"` の `UserMessage` として記録される)。
    pub async fn fork(
        &self,
        command_id: &str,
        session_id: &str,
        from_node_seq: Option<u64>,
        agent_id: Option<&str>,
        new_cwd: Option<&Path>,
    ) -> Result<EnsureSessionOutcome, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let source = self
            .inner
            .bus
            .db()
            .get_session(session_id)
            .await?
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;

        let events = self.load_session_events(session_id, from_node_seq).await?;
        let fork_seq = events
            .last()
            .map(|event| event.node_seq)
            .or(source.fork_from_node_seq)
            .unwrap_or(1);

        let spec = self
            .resolve_launch_spec(agent_id.unwrap_or(&source.agent_id), &[])
            .await?;
        let project_name = self.project_name(&source.project_id).await;

        let new_session_id = uuid_v7();
        let cwd = new_cwd
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(&source.local_path));
        let (git_branch, is_worktree) = branch_and_worktree(&cwd).await;
        let title = format!("Fork of {}", source.title);
        self.inner
            .bus
            .create_session(
                &new_session_id,
                UnifiedEventPayload::SessionCreated {
                    node_id: self.inner.node_id.clone(),
                    project_id: source.project_id.clone(),
                    project_name,
                    local_path: cwd.to_string_lossy().into_owned(),
                    git_branch,
                    is_worktree,
                    agent_id: spec.agent_id.clone(),
                    parent_session_id: Some(source.session_id.clone()),
                    fork_from_node_seq: Some(fork_seq),
                    title: title.clone(),
                },
            )
            .await?;

        let handle = self
            .start_driver_session(StartDriverParams {
                session_id: &new_session_id,
                title: &title,
                cwd: &cwd,
                spec: &spec,
                extra_args: Vec::new(),
                initial_mode: None,
                has_custom_title: true,
            })
            .await?;

        // 履歴 Replay 注入 (新エージェントセッションの初期コンテキスト)
        if let Some(context) = build_fork_context(&events, fork_seq)
            && let Err(err) = self.start_turn(&new_session_id, &context, "fork").await
        {
            tracing::warn!(
                session_id = %new_session_id,
                "failed to inject fork context: {err:#}"
            );
        }

        Ok(EnsureSessionOutcome {
            session_id: new_session_id,
            attach_mode: attach_mode_of(handle.as_ref()),
        })
    }

    /// 稼働中セッションの CLI アタッチモードを返す (`fxg attach`)。
    ///
    /// 停止済み・未知のセッションは [`AttachMode::AcpTui`] (イベント履歴の閲覧)。
    pub fn attach_mode(&self, session_id: &str) -> AttachMode {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        sessions
            .active
            .get(session_id)
            .map(|session| attach_mode_of(session.handle.as_ref()))
            .unwrap_or(AttachMode::AcpTui)
    }

    /// 論理プロジェクトの表示名を解決する (未知の場合は `project_id` を返す)。
    async fn project_name(&self, project_id: &str) -> String {
        match self.inner.bus.db().list_projects().await {
            Ok(projects) => projects
                .into_iter()
                .find(|project| project.project_id == project_id)
                .map(|project| project.name)
                .unwrap_or_else(|| project_id.to_owned()),
            Err(_) => project_id.to_owned(),
        }
    }

    /// ターンを開始する (busy 予約 → スナップショット → `UserMessage` → 送信)。
    ///
    /// 呼び出し側は事前に busy 判定 (Pending Queue への追加 or 拒否) を済ませる。
    async fn start_turn(
        &self,
        session_id: &str,
        text: &str,
        client_source: &str,
    ) -> Result<(), NodeError> {
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions
                .active
                .get_mut(session_id)
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
            session.busy = true;
        }
        match dispatch_prompt(&self.inner, session_id, text, client_source).await {
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
        }

        self.start_turn(session_id, text, client_source).await
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
                    UnifiedEventPayload::PermissionResolved { request_id, .. } => {
                        // ドライバ側 (OpenCode2 純正TUI 等) で解決された承認もここに来る。
                        // fxg 経由の応答は `SessionManager::respond_permission` が
                        // 記録済みのため、承認待ち一覧に残っている場合のみ記録する。
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        let already_recorded =
                            sessions.active.get_mut(&session_id).is_some_and(|session| {
                                session.pending_permissions.remove(request_id).is_none()
                            });
                        if already_recorded {
                            continue;
                        }
                    }
                    UnifiedEventPayload::SessionTitleChanged { .. } => {
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        if let Some(session) = sessions.active.get_mut(&session_id) {
                            session.has_custom_title = true;
                        }
                    }
                    _ => {}
                }
                match inner.bus.record(&session_id, payload).await {
                    Ok(_) => maybe_dispatch_queued(&inner, &session_id).await,
                    Err(err) => {
                        tracing::warn!(session_id = %session_id, "failed to persist driver event: {err:#}");
                    }
                }
            }
        }
    }
    // チャネルが閉じた = ドライバ (エージェントプロセス) 終了。
    // `Stopped` への遷移は DB の `sessions` 投影にも反映する必要があるため
    // イベントとして追記する (既に Stopped / Error の場合はそのまま)。
    let previous = {
        let sessions = inner.sessions.lock().expect("sessions poisoned");
        sessions
            .active
            .get(&session_id)
            .map(|session| session.status)
    };
    mark_status(&inner, &session_id, SessionStatus::Stopped, None);
    if matches!(
        previous,
        Some(
            SessionStatus::Provisioning
                | SessionStatus::Bootstrapping
                | SessionStatus::Idle
                | SessionStatus::Running
                | SessionStatus::WaitingPermission
        )
    ) {
        let _ = inner
            .bus
            .record(
                &session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Stopped,
                    error_message: None,
                },
            )
            .await;
    }
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

    // busy は呼び出し元で予約済み。失敗時は解除して整合性を保つ。
    if let Err(err) = dispatch_prompt(inner, session_id, &prompt.text, &prompt.client_source).await
    {
        tracing::warn!(session_id = %session_id, "failed to dispatch queued prompt: {err:#}");
        let _ = inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Error,
                    error_message: Some(format!("failed to dispatch queued prompt: {err:#}")),
                },
            )
            .await;
        if let Some(session) = inner
            .sessions
            .lock()
            .expect("sessions poisoned")
            .active
            .get_mut(session_id)
        {
            session.busy = false;
        }
    }
}

/// プロンプトの先頭部分から人間が読みやすいセッションタイトルを導出する。
///
/// - 先頭の空白・空行・Markdown記号 (`#`, `-`, `*`, `>`, バッククォート等) をトリムする
/// - 最初の1行を取り出す (最大40文字、超過時は `...` を付加)
/// - 有効な文字列が抽出できない場合は `None` を返す
pub fn derive_title_from_prompt(prompt: &str) -> Option<String> {
    for line in prompt.lines() {
        let trimmed = line.trim();
        let cleaned = trimmed.trim_start_matches(['#', '>', '-', '*', '`']).trim();
        if !cleaned.is_empty() {
            let char_count = cleaned.chars().count();
            if char_count <= 40 {
                return Some(cleaned.to_owned());
            } else {
                let truncated: String = cleaned.chars().take(40).collect();
                return Some(format!("{truncated}..."));
            }
        }
    }
    None
}

/// 1 ターンを開始する (スナップショット → `UserMessage` 記録 → ドライバ送信)。
///
/// busy の予約・解除は呼び出し側 (`SessionManager::start_turn` /
/// [`maybe_dispatch_queued`]) が行う。
async fn dispatch_prompt(
    inner: &Arc<Inner>,
    session_id: &str,
    text: &str,
    client_source: &str,
) -> Result<(), NodeError> {
    let (cwd, should_update_title) = {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        let session = sessions
            .active
            .get_mut(session_id)
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;
        let should_update = !session.has_custom_title;
        if should_update {
            session.has_custom_title = true;
        }
        (session.cwd.clone(), should_update)
    };

    if should_update_title && let Some(title) = derive_title_from_prompt(text) {
        inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::SessionTitleChanged { title },
            )
            .await?;
    }

    let snapshot_tree_hash = snapshot_tree_hash(
        &cwd,
        &inner.paths.snapshot_index_path(session_id),
        session_id,
    )
    .await;

    inner
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
        let sessions = inner.sessions.lock().expect("sessions poisoned");
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

/// 読み込み済みイベントから Revert 基準のスナップショットを選ぶ。
///
/// 最後の `UserMessage.snapshot_tree_hash` を返す (`node_seq`, Tree Hash)。
fn find_revert_snapshot(events: &[SessionEventEnvelope]) -> Option<(u64, String)> {
    events.iter().rev().find_map(|event| match &event.payload {
        UnifiedEventPayload::UserMessage {
            snapshot_tree_hash: Some(tree_hash),
            ..
        } => Some((event.node_seq, tree_hash.clone())),
        _ => None,
    })
}

/// Fork 時に新エージェントセッションへ注入する会話履歴コンテキストを組み立てる。
///
/// 会話 (ユーザー / エージェントメッセージ) と、ツール呼び出しの要約・変更ファイルを
/// `node_seq` 順に連結する。会話が無い (履歴なし) 場合は `None`。
fn build_fork_context(events: &[SessionEventEnvelope], fork_seq: u64) -> Option<String> {
    /// 注入するコンテキストの最大文字数 (過大なプロンプトを防ぐ)。
    const MAX_CONTEXT_CHARS: usize = 16_000;

    let mut parts: Vec<String> = Vec::new();
    for event in events {
        if event.node_seq > fork_seq {
            break;
        }
        match &event.payload {
            UnifiedEventPayload::UserMessage { text, .. } => {
                parts.push(format!("### User\n{text}"));
            }
            UnifiedEventPayload::AgentMessage {
                text,
                is_complete: true,
                ..
            } => {
                parts.push(format!("### Assistant\n{text}"));
            }
            UnifiedEventPayload::ToolCall {
                title,
                status,
                locations,
                diff,
                ..
            } => {
                let mut line = format!("- [{status}] {title}");
                if !locations.is_empty() {
                    line.push_str(&format!(" ({})", locations.join(", ")));
                }
                if let Some(diff) = diff {
                    line.push_str(&format!(
                        " — {} (+{}/-{})",
                        diff.path, diff.additions, diff.deletions
                    ));
                }
                parts.push(line);
            }
            _ => {}
        }
    }

    if parts.is_empty() {
        return None;
    }

    let mut context = format!(
        "以下の履歴は、以前のセッション (node_seq <= {fork_seq}) をこの時点から \
         Fork したものです。この文脈を引き継いで作業を続けてください。\n\n"
    );
    for part in parts {
        if context.chars().count() + part.chars().count() > MAX_CONTEXT_CHARS {
            context.push_str("\n(履歴は上限に達したため省略されました)");
            break;
        }
        context.push_str(&part);
        context.push_str("\n\n");
    }
    Some(context)
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

    /// `busy = false` になるまで待つ (ターン完了待ち)。
    async fn wait_not_busy(manager: &SessionManager, session_id: &str) {
        for _ in 0..200 {
            let busy = manager
                .list_active()
                .iter()
                .find(|session| session.session_id == session_id)
                .map(|session| session.busy);
            match busy {
                Some(false) | None => return,
                _ => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
        panic!("session did not finish its turn");
    }

    /// 指定テキストの `UserMessage` が永続化されるまで待ち、`node_seq` を返す。
    async fn wait_user_message(manager: &SessionManager, session_id: &str, text: &str) -> u64 {
        for _ in 0..200 {
            let batch = manager
                .bus()
                .db()
                .session_events_after(session_id, 0, 100)
                .await
                .expect("events");
            if let Some(event) = batch.events.iter().find(|event| {
                matches!(&event.payload, UnifiedEventPayload::UserMessage { text: body, .. } if body == text)
            }) {
                return event.node_seq;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("UserMessage was not recorded: {text}");
    }

    /// 指定 `event_type` のイベントが永続化されるまで待つ。
    async fn wait_event_type(manager: &SessionManager, session_id: &str, event_type: &str) {
        for _ in 0..200 {
            let batch = manager
                .bus()
                .db()
                .session_events_after(session_id, 0, 100)
                .await
                .expect("events");
            if batch
                .events
                .iter()
                .any(|event| event.payload.event_type() == event_type)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("event was not recorded: {event_type}");
    }

    /// テスト用に Git リポジトリ (`repo`) を用意し、セッションを開始する。
    async fn setup_repo_session(
        manager: &SessionManager,
        dir: &tempfile::TempDir,
    ) -> (PathBuf, String) {
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        crate::testutil::init_test_repo(&repo).await;
        let outcome = manager
            .ensure_session("c1", &repo, "mock", &[], None, false)
            .await
            .expect("ensure");
        wait_idle(manager, &outcome.session_id).await;
        (repo, outcome.session_id)
    }

    #[tokio::test]
    async fn ensure_session_records_created_event_and_start() {
        let (manager, _mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
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
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
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
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
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
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
            .await
            .expect("ensure");
        let err = manager
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
            .await
            .expect_err("duplicate");
        assert!(matches!(err, NodeError::CommandDuplicate(_)));
    }

    #[tokio::test]
    async fn permission_resolution_is_idempotent() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
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
    async fn revert_restores_workspace_files() {
        let (manager, mock, dir) = setup().await;
        let (repo, session_id) = setup_repo_session(&manager, &dir).await;

        // ターン開始時のスナップショットを記録する
        manager
            .send_prompt("c2", &session_id, "first turn", "cli")
            .await
            .expect("prompt");
        let user_seq = wait_user_message(&manager, &session_id, "first turn").await;

        // ターン完了 → エージェントがファイルを変更した想定
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(&manager, &session_id).await;
        std::fs::write(repo.join("README.md"), "changed by agent\n").expect("write");

        // Revert: 直近ターン開始時点のファイル状態へ復元する
        let outcome = manager
            .revert("c3", &session_id, None)
            .await
            .expect("revert");
        assert_eq!(outcome.target_node_seq, user_seq);
        assert!(outcome.restored_files >= 1);
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).expect("read"),
            "# test\n"
        );

        // ドライバの revert_context が呼ばれる (対応ドライバ)。
        // 引数は「先頭から残すターン数」= target_seq 以前のユーザーメッセージ数。
        assert_eq!(mock.reverted(), vec![1]);

        // SessionReverted がイベントログに追記される
        let batch = manager
            .bus()
            .db()
            .session_events_after(&session_id, 0, 100)
            .await
            .expect("events");
        let reverted = batch
            .events
            .iter()
            .find(|event| {
                matches!(&event.payload, UnifiedEventPayload::SessionReverted { target_node_seq, .. } if *target_node_seq == user_seq)
            })
            .expect("session_reverted event");
        match &reverted.payload {
            UnifiedEventPayload::SessionReverted {
                restored_tree_hash,
                backup_tree_hash,
                ..
            } => {
                assert!(!restored_tree_hash.is_empty());
                assert!(backup_tree_hash.is_some(), "バックアップ Tree が退避される");
            }
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn revert_rejects_busy_session() {
        let (manager, _mock, dir) = setup().await;
        let (_repo, session_id) = setup_repo_session(&manager, &dir).await;

        // ターン実行中 (Idle イベント未受信) は Busy で拒否する
        manager
            .send_prompt("c2", &session_id, "running", "cli")
            .await
            .expect("prompt");
        let err = manager
            .revert("c3", &session_id, None)
            .await
            .expect_err("busy");
        assert!(matches!(err, NodeError::Busy(_)));
    }

    #[tokio::test]
    async fn fork_creates_child_session_and_injects_history() {
        let (manager, mock, dir) = setup().await;
        let (repo, source_id) = setup_repo_session(&manager, &dir).await;

        manager
            .send_prompt("c2", &source_id, "hello world", "cli")
            .await
            .expect("prompt");
        let user_seq = wait_user_message(&manager, &source_id, "hello world").await;
        mock.emit(DriverEvent::Event(UnifiedEventPayload::AgentMessage {
            message_id: "m1".to_owned(),
            text: "了解しました".to_owned(),
            is_complete: true,
        }));
        wait_event_type(&manager, &source_id, "agent_message").await;
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(&manager, &source_id).await;

        // `user_seq` 時点から分岐する
        let fork = manager
            .fork("c3", &source_id, Some(user_seq), None, None)
            .await
            .expect("fork");
        assert_ne!(fork.session_id, source_id);

        // 親子関係が記録される
        let child = manager
            .bus()
            .db()
            .get_session(&fork.session_id)
            .await
            .expect("query")
            .expect("child row");
        assert_eq!(child.parent_session_id.as_deref(), Some(source_id.as_str()));
        assert_eq!(child.fork_from_node_seq, Some(user_seq));
        assert_eq!(child.agent_id, "mock");

        // 履歴 Replay 注入: fork セッションの UserMessage として記録され、
        // `user_seq` 時点までの会話 (ユーザー発話) が含まれる
        let batch = manager
            .bus()
            .db()
            .session_events_after(&fork.session_id, 0, 100)
            .await
            .expect("events");
        let injected = batch
            .events
            .iter()
            .find_map(|event| match &event.payload {
                UnifiedEventPayload::UserMessage {
                    text,
                    client_source,
                    ..
                } if client_source == "fork" => Some(text.clone()),
                _ => None,
            })
            .expect("injected fork context");
        assert!(
            injected.contains("hello world"),
            "履歴が含まれる: {injected}"
        );
        assert!(
            !injected.contains("了解しました"),
            "分岐点より後の会話は含まれない: {injected}"
        );
        assert!(
            mock.prompts().iter().any(|prompt| prompt == &injected),
            "コンテキストが新エージェントへ送信される"
        );

        // 分岐先の作業ディレクトリは同じリポジトリを共有する
        // (macOS の `/var` → `/private/var` 解決があるため canonicalize して比較)
        assert_eq!(
            PathBuf::from(&child.local_path)
                .canonicalize()
                .expect("canonicalize child"),
            repo.canonicalize().expect("canonicalize repo")
        );
    }

    #[tokio::test]
    async fn driver_exit_records_stopped_status() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        // ドライバ (エージェントプロセス) 終了: イベントチャネルが閉じると
        // ポンプが `Stopped` を記録し、active 一覧から外れる
        mock.close_events();
        for _ in 0..200 {
            let row = manager
                .bus()
                .db()
                .get_session(&outcome.session_id)
                .await
                .expect("query");
            if row.is_some_and(|session| session.status == SessionStatus::Stopped) {
                assert!(
                    manager.list_active().is_empty(),
                    "停止したセッションは active 一覧から外れる"
                );
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("driver exit did not record Stopped");
    }

    #[tokio::test]
    async fn session_status_transitions_on_driver_events() {
        let (manager, mock, dir) = setup().await;
        manager
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
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

    #[test]
    fn derives_title_from_prompts() {
        assert_eq!(
            derive_title_from_prompt("Hello world"),
            Some("Hello world".to_owned())
        );
        assert_eq!(
            derive_title_from_prompt("# Task title\nSome description here"),
            Some("Task title".to_owned())
        );
        assert_eq!(
            derive_title_from_prompt("   \n\n> Blockquote prompt\nNext line"),
            Some("Blockquote prompt".to_owned())
        );
        let long = "あ".repeat(50);
        let expected = format!("{}...", "あ".repeat(40));
        assert_eq!(derive_title_from_prompt(&long), Some(expected));
        assert_eq!(derive_title_from_prompt("   \n\n  "), None);
    }

    #[tokio::test]
    async fn start_session_uses_initial_prompt_for_title() {
        let (manager, _mock, dir) = setup().await;
        let session_id = fxg_protocol::util::uuid_v7();
        manager
            .start_session(StartSessionParams {
                command_id: "c_init",
                session_id: &session_id,
                local_path: dir.path(),
                agent_id: "mock",
                initial_prompt: Some("# First Turn\nDo something"),
            })
            .await
            .expect("start");

        let row = manager
            .bus()
            .db()
            .get_session(&session_id)
            .await
            .expect("query")
            .expect("session");
        assert_eq!(row.title, "First Turn");
    }

    #[tokio::test]
    async fn first_prompt_updates_default_title() {
        let (manager, _mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c_ensure", dir.path(), "mock", &[], None, false)
            .await
            .expect("ensure");

        // 初期タイトルはデフォルト
        let initial_row = manager
            .bus()
            .db()
            .get_session(&outcome.session_id)
            .await
            .expect("query")
            .expect("session");
        assert!(initial_row.title.contains('@'));

        // 最初のプロンプトを送信
        manager
            .send_prompt(
                "c_prompt1",
                &outcome.session_id,
                "Implement login page",
                "web",
            )
            .await
            .expect("prompt");

        // タイトルが更新される
        let updated_row = manager
            .bus()
            .db()
            .get_session(&outcome.session_id)
            .await
            .expect("query")
            .expect("session");
        assert_eq!(updated_row.title, "Implement login page");

        // 2回目のプロンプトではタイトルは再更新されない (初回のみ)
        manager
            .send_prompt(
                "c_prompt2",
                &outcome.session_id,
                "Now add logout button",
                "web",
            )
            .await
            .expect("prompt");

        let second_row = manager
            .bus()
            .db()
            .get_session(&outcome.session_id)
            .await
            .expect("query")
            .expect("session");
        assert_eq!(second_row.title, "Implement login page");
    }
}
