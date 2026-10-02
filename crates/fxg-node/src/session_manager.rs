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

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fxg_acp::registry::{AcpRegistry, OPENCODE2_ID, RegistryIndex};
use fxg_acp::{
    AcpDriver, ActiveSessionHandle, AgentDriver, AgentLaunchSpec, DriverEvent,
    NativeResumeUnavailable, OpenCode2Driver, ResumeRequest, ensure_opencode_v2,
};
use fxg_protocol::common::{
    ElicitationAction, ForkHistoryItem, PermissionOption, SessionControlAction, SessionStatus,
};
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

/// 送信時の自動レジューム (ネイティブ復元) の完了待ち上限。
///
/// コールドスタートのエージェント (PyInstaller 展開等で ~25 秒) を許容する。
const AUTO_RESUME_TIMEOUT: Duration = Duration::from_secs(120);

/// 他の再開処理 (明示 resume / 自動レジューム) の完了を待つポーリング間隔。
const RESUME_WAIT_INTERVAL: Duration = Duration::from_millis(100);

/// 削除前のセッション停止 (エージェントプロセス終了) の待機上限。
const STOP_BEFORE_DELETE_TIMEOUT: Duration = Duration::from_secs(30);

/// エージェント起動スペックからドライバを選択するファクトリ。
///
/// ドライバインスタンスはデーモンで共有される (セッションごとに生成しない)。
/// `AcpDriver` はセッション終了後のプロセスを再利用プールへ保持するため、
/// インスタンスが分かれるとプールが機能しない。
pub type DriverFactory =
    Arc<dyn Fn(&AgentLaunchSpec) -> anyhow::Result<Arc<dyn AgentDriver>> + Send + Sync>;

/// 既定のドライバファクトリ。
///
/// - `acp`: 標準ACPエージェント ([`AcpDriver`])
/// - `opencode2`: `opencode serve` ブリッジ ([`OpenCode2Driver`])
///
/// ドライバは種類ごとに 1 個を生成し、全セッションで共有する。
pub fn default_driver_factory() -> DriverFactory {
    let acp: Arc<dyn AgentDriver> = Arc::new(AcpDriver::new());
    let opencode2: Arc<dyn AgentDriver> = Arc::new(OpenCode2Driver::new());
    Arc::new(
        move |spec: &AgentLaunchSpec| match spec.driver_kind.as_str() {
            "acp" => Ok(Arc::clone(&acp)),
            "opencode2" => Ok(Arc::clone(&opencode2)),
            other => Err(anyhow::anyhow!("unknown driver kind: {other}")),
        },
    )
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

/// `opencode2` の起動モード (`"bridge"` / `"acp"`) を起動スペックへ適用する。
///
/// `opencode_mode` が `None` の場合はレジストリ既定 (config の
/// `opencode_mode`) を尊重してスペックを変更しない。`SessionCreated` へ
/// 永続化する実効モードを返す (`opencode2` 以外は `None`)。
fn apply_opencode_mode(
    spec: &mut AgentLaunchSpec,
    opencode_mode: Option<&str>,
    extra_args: &[String],
) -> Option<String> {
    if spec.agent_id != OPENCODE2_ID {
        return None;
    }
    if let Some(mode) = opencode_mode {
        if mode == "acp" {
            spec.driver_kind = "acp".to_owned();
            spec.args = vec!["acp".to_owned()];
        } else {
            spec.driver_kind = "opencode2".to_owned();
            spec.args = vec!["serve".to_owned()];
        }
        spec.args.extend(extra_args.iter().cloned());
    }
    Some(
        if spec.driver_kind == "acp" {
            "acp"
        } else {
            "bridge"
        }
        .to_owned(),
    )
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
    /// 初期モード (`code` / `plan` 等)
    pub mode: Option<&'a str>,
    /// OpenCode2 起動モード ("bridge" または "acp")
    pub opencode_mode: Option<&'a str>,
    /// エージェントへの追加パススルー引数
    pub extra_args: Option<&'a [String]>,
    /// Fork 時の履歴 Replay 注入 (別ノード・一時VMからの引き継ぎ)
    pub fork_context: Option<&'a [ForkHistoryItem]>,
    /// 別ノード・一時VMから退避された Git バンドルの復元 (ローカルファイルパス)
    pub restore_git_bundle: Option<&'a Path>,
}

/// 停止済みセッションの再開要求 ([`SessionManager::resume`])。
///
/// 同一 `session_id` のままエージェントを起動し、ネイティブ復元
/// (`session/resume` → `session/load` → OpenCode2 既存セッション bind) を
/// 優先する。復元できない場合は履歴 Replay を注入して継続する
/// (設計: `docs/04-agent-drivers-and-windows.md` §4.3)。
#[derive(Debug, Clone, Copy)]
pub struct ResumeParams<'a> {
    /// 相関ID (重複送信の冪等排除)
    pub command_id: &'a str,
    /// 対象セッションID
    pub session_id: &'a str,
    /// ネイティブ復元を試みず履歴 Replay で継続する
    pub force_replay: bool,
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

/// `resume` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct ResumeOutcome {
    /// 再開したセッションID (リクエスト対象と同一)
    pub session_id: String,
    /// CLI のアタッチモード
    pub attach_mode: AttachMode,
    /// ネイティブ復元できたか (`false` = 履歴 Replay で継続)
    pub context_restored: bool,
}

/// [`SessionManager::resume_stopped_session`] の復元方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeMode {
    /// ネイティブ復元を優先し、不可なら新規セッションを作成する
    /// (呼び出し側が履歴 Replay を注入する)。
    AllowReplay {
        /// 記録済みエージェントセッションIDを使わず新規作成させる (`--replay`)
        force_replay: bool,
    },
    /// ネイティブ復元のみ。不可なら [`NodeError::ResumeRequired`] を返す
    /// (プロンプト送信時の自動レジューム)。
    NativeOnly,
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

/// `fxg inbox` 用の回答待ち elicitation エントリ。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingElicitationSummary {
    /// 対象セッションID
    pub session_id: String,
    /// ACP elicitation id (form モードは JSON-RPC request id)
    pub elicitation_id: String,
    /// ユーザーへ提示するメッセージ
    pub message: String,
    /// 要求モード (`form` のみ対応)
    pub mode: String,
    /// form モードの要求 schema
    pub requested_schema: serde_json::Value,
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
    /// ファクトリが返したドライバ (共有インスタンス。`shutdown_idle` 用に保持)
    drivers: Mutex<Vec<Arc<dyn AgentDriver>>>,
}

#[derive(Default)]
struct Sessions {
    active: HashMap<String, ActiveSession>,
    /// 直近に処理した `command_id` (古いものから破棄)
    processed_commands: VecDeque<String>,
    /// 再開処理中 (二重レジュームの排他) のセッションID
    resuming: HashSet<String>,
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
    /// 回答待ち elicitation (`elicitation_id` → 表示用情報)
    pending_elicitations: HashMap<String, PendingElicitationSummary>,
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
    /// 既存エージェントセッションからの再開指定 (`None` は新規セッション)
    resume: Option<ResumeRequest>,
}

/// [`SessionManager::start_driver_session`] の結果 (ハンドル + 復元成否)。
struct StartedDriverSession {
    /// 操作ハンドル
    handle: Arc<dyn ActiveSessionHandle>,
    /// `true` = エージェント側コンテキストをネイティブ復元した
    /// (`false` は呼び出し側が履歴 Replay を注入する)
    context_restored: bool,
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
                drivers: Mutex::new(Vec::new()),
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

    /// ファクトリが返した共有ドライバを記録する (重複はポインタで排除)。
    ///
    /// `shutdown_all` がアイドルプロセス (warm プール) を破棄するために使う。
    fn remember_driver(&self, driver: &Arc<dyn AgentDriver>) {
        let mut drivers = self.inner.drivers.lock().expect("drivers poisoned");
        if !drivers.iter().any(|existing| Arc::ptr_eq(existing, driver)) {
            drivers.push(Arc::clone(driver));
        }
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
        let opencode_mode = apply_opencode_mode(&mut spec, acp.then_some("acp"), extra_args);

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
                    opencode_mode,
                },
            )
            .await?;

        let started = self
            .start_driver_session(StartDriverParams {
                session_id: &session_id,
                title: &title,
                cwd: &resolved.local_path,
                spec: &spec,
                extra_args: extra_args.to_vec(),
                initial_mode: initial_mode.map(str::to_owned),
                has_custom_title: false,
                resume: None,
            })
            .await?;

        Ok(EnsureSessionOutcome {
            session_id,
            attach_mode: attach_mode_of(started.handle.as_ref()),
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

        // 退避済み Git バンドルの復元 (別ノードへの引き継ぎ)。
        // プロジェクト解決より前にワークスペースを実体化する。
        if let Some(bundle_file) = params.restore_git_bundle {
            crate::bundle::restore_workspace_bundle(params.local_path, bundle_file).await?;
        }

        let resolved = project::resolve_project(params.local_path, &self.inner.node_id).await?;
        let extra_args_vec: Vec<String> = params.extra_args.map(|s| s.to_vec()).unwrap_or_default();
        let mut spec = self
            .resolve_launch_spec(params.agent_id, &extra_args_vec)
            .await?;
        let opencode_mode = apply_opencode_mode(&mut spec, params.opencode_mode, &extra_args_vec);

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
                    opencode_mode,
                },
            )
            .await?;

        let started = self
            .start_driver_session(StartDriverParams {
                session_id: params.session_id,
                title: &title,
                cwd: &resolved.local_path,
                spec: &spec,
                extra_args: extra_args_vec,
                initial_mode: params.mode.map(str::to_owned),
                has_custom_title,
                resume: None,
            })
            .await?;

        // 初期プロンプト (リモートからのセッション起動時にそのまま実行する)
        if let Some(prompt) = params.initial_prompt.filter(|text| !text.trim().is_empty()) {
            self.start_turn(params.session_id, prompt, "web").await?;
        }

        // 別ノード・一時VMからの Fork: 履歴 Replay を初期コンテキストとして注入する
        if let Some(context) = params
            .fork_context
            .and_then(|items| build_history_replay_context_from_items(items, ReplayPurpose::Fork))
            && let Err(err) = self.start_turn(params.session_id, &context, "fork").await
        {
            tracing::warn!(
                session_id = %params.session_id,
                "failed to inject fork context: {err:#}"
            );
        }

        Ok(EnsureSessionOutcome {
            session_id: params.session_id.to_owned(),
            attach_mode: attach_mode_of(started.handle.as_ref()),
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
    /// アタッチモード決定に使うハンドルとネイティブ復元成否を返す。
    async fn start_driver_session(
        &self,
        params: StartDriverParams<'_>,
    ) -> Result<StartedDriverSession, NodeError> {
        // Git Credential Proxy (一時VM向け GIT_ASKPASS) をエージェントプロセスへ注入する。
        // スクリプトはデーモン起動時に生成済みで、`fxg git-askpass` → ローカルIPC →
        // 中央サーバーのオンメモリ中継で認証する (設計: docs/01 §6.4)。
        let mut spec = params.spec.clone();
        // opencode は v1 / v2 が同名コマンドのため、`serve` API を持たない v1 を
        // 誤って起動しないよう、起動前に v2 系であることを検証する。
        if spec.agent_id == OPENCODE2_ID
            && let Err(err) = ensure_opencode_v2(&spec.program.to_string_lossy(), params.cwd).await
        {
            let message = format!("{err:#}");
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
        let askpass = crate::credentials::askpass_script_path(self.inner.paths.fxg_home());
        if askpass.exists() {
            for (key, value) in crate::credentials::git_env(self.inner.paths.fxg_home()) {
                spec.env.retain(|(k, _)| k != &key);
                spec.env.push((key, value));
            }
        }
        let params = StartDriverParams {
            spec: &spec,
            ..params
        };

        let driver =
            (self.inner.factory)(params.spec).map_err(|err| NodeError::Agent(err.to_string()))?;
        self.remember_driver(&driver);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let started = match driver
            .start_session(
                fxg_acp::StartSessionRequest {
                    session_id: params.session_id.to_owned(),
                    title: Some(params.title.to_owned()),
                    cwd: params.cwd.to_path_buf(),
                    launch: params.spec.clone(),
                    extra_args: params.extra_args,
                    initial_mode: params.initial_mode,
                    resume: params.resume,
                },
                event_tx,
            )
            .await
        {
            Ok(started) => started,
            Err(err) => {
                // ネイティブ限定の自動レジューム (`allow_fresh = false`) では、
                // 「ネイティブ復元不可」をセッション停止状態のまま呼び出し側へ返す
                // (履歴 Replay は暗黙実行せず、明示的な Resume を促す)
                if let Some(unavailable) = err.downcast_ref::<NativeResumeUnavailable>() {
                    tracing::info!(
                        session_id = params.session_id,
                        "native resume is not available: {unavailable}"
                    );
                    return Err(NodeError::ResumeRequired(format!(
                        "{}: {}",
                        params.session_id, unavailable.0
                    )));
                }
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

        let context_restored = started.context_restored;
        let handle: Arc<dyn ActiveSessionHandle> = Arc::from(started.handle);
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
                    pending_elicitations: HashMap::new(),
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
        Ok(StartedDriverSession {
            handle,
            context_restored,
        })
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

        let mut spec = self
            .resolve_launch_spec(agent_id.unwrap_or(&source.agent_id), &[])
            .await?;
        // 実効の opencode2 起動モードを記録する (スペックは変更しない)
        let opencode_mode = apply_opencode_mode(&mut spec, None, &[]);
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
                    opencode_mode,
                },
            )
            .await?;

        let started = self
            .start_driver_session(StartDriverParams {
                session_id: &new_session_id,
                title: &title,
                cwd: &cwd,
                spec: &spec,
                extra_args: Vec::new(),
                initial_mode: None,
                has_custom_title: true,
                resume: None,
            })
            .await?;

        // 履歴 Replay 注入 (新エージェントセッションの初期コンテキスト)
        if let Some(context) = build_history_replay_context(&events, fork_seq, ReplayPurpose::Fork)
            && let Err(err) = self.start_turn(&new_session_id, &context, "fork").await
        {
            tracing::warn!(
                session_id = %new_session_id,
                "failed to inject fork context: {err:#}"
            );
        }

        Ok(EnsureSessionOutcome {
            session_id: new_session_id,
            attach_mode: attach_mode_of(started.handle.as_ref()),
        })
    }

    /// 停止済みセッションを再開する (`fxg session resume`)。
    ///
    /// - 同一 `session_id` のままエージェントを起動し、ネイティブ復元
    ///   (`session/resume` → `session/load` → OpenCode2 既存セッション bind) を
    ///   優先する。復元できなかった場合は履歴 Replay を注入して継続する。
    /// - 再開可否は `sessions.active` (実際の稼働状況) を正とする。
    ///   強制終了時は `stopped` への遷移イベントが書かれないため、DB の
    ///   `status` が `idle` / `running` のままでも再開を許可する。
    /// - 一時VM (`provisioning` / `bootstrapping`) セッションは v1 では
    ///   再開できない (設計: `docs/04-agent-drivers-and-windows.md` §4.3)。
    pub async fn resume(&self, params: ResumeParams<'_>) -> Result<ResumeOutcome, NodeError> {
        if !self.begin_command(params.command_id) {
            return Err(NodeError::CommandDuplicate(params.command_id.to_owned()));
        }

        // 二重レジュームの排他 (active + 起動処理中の双方を拒否する)
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            if sessions.active.contains_key(params.session_id)
                || !sessions.resuming.insert(params.session_id.to_owned())
            {
                return Err(NodeError::InvalidState(format!(
                    "session is already active or resuming: {}",
                    params.session_id
                )));
            }
        }
        let result = self.resume_session_state(params).await;
        self.inner
            .sessions
            .lock()
            .expect("sessions poisoned")
            .resuming
            .remove(params.session_id);
        result
    }

    /// [`Self::resume`] の本体 (排他ガードの解除は呼び出し側で行う)。
    async fn resume_session_state(
        &self,
        params: ResumeParams<'_>,
    ) -> Result<ResumeOutcome, NodeError> {
        let (started, events) = self
            .resume_stopped_session(
                params.session_id,
                ResumeMode::AllowReplay {
                    force_replay: params.force_replay,
                },
            )
            .await?;

        // ネイティブ復元できなかった場合は履歴 Replay でコンテキストを継続する
        if !started.context_restored {
            let up_to_seq = events.last().map(|event| event.node_seq).unwrap_or(0);
            if let Some(context) =
                build_history_replay_context(&events, up_to_seq, ReplayPurpose::Resume)
                && let Err(err) = self.start_turn(params.session_id, &context, "resume").await
            {
                tracing::warn!(
                    session_id = params.session_id,
                    "failed to inject resume context: {err:#}"
                );
            }
        }

        Ok(ResumeOutcome {
            session_id: params.session_id.to_owned(),
            attach_mode: attach_mode_of(started.handle.as_ref()),
            context_restored: started.context_restored,
        })
    }

    /// 停止済みセッションを検証し、ドライバを起動して `StatusChanged(Idle)` まで
    /// 記録する (明示 resume / 送信時自動レジューム共通)。
    ///
    /// - 一時VM (`provisioning` / `bootstrapping`) と作業ディレクトリ消滅は
    ///   [`NodeError::InvalidState`] で拒否する。
    /// - [`ResumeMode::NativeOnly`] でネイティブ復元できない場合、ドライバは
    ///   [`NativeResumeUnavailable`] を返し、[`NodeError::ResumeRequired`] として
    ///   呼び出し側へ伝わる (セッションは停止状態のまま残る)。
    ///
    /// 戻り値は起動済みセッションと読み込み済みイベント (Replay 判定用)。
    async fn resume_stopped_session(
        &self,
        session_id: &str,
        mode: ResumeMode,
    ) -> Result<(StartedDriverSession, Vec<SessionEventEnvelope>), NodeError> {
        let session = self
            .inner
            .bus
            .db()
            .get_session(session_id)
            .await?
            .ok_or_else(|| NodeError::InvalidSession(session_id.to_owned()))?;

        // 一時VM (provisioner) セッションの再開は v2 (未対応)
        if matches!(
            session.status,
            SessionStatus::Provisioning | SessionStatus::Bootstrapping
        ) {
            return Err(NodeError::InvalidState(format!(
                "resuming ephemeral sessions is not supported yet: {session_id}"
            )));
        }

        // 作業ディレクトリが消えている (Worktree 削除済み等) 場合は再開不可
        let cwd = PathBuf::from(&session.local_path);
        if !cwd.is_dir() {
            return Err(NodeError::InvalidState(format!(
                "session workspace does not exist: {}",
                session.local_path
            )));
        }

        let events = self.load_session_events(session_id, None).await?;
        // 起動モードは SessionCreated イベントを正として復元する
        let opencode_mode = events.iter().find_map(|event| match &event.payload {
            UnifiedEventPayload::SessionCreated { opencode_mode, .. } => opencode_mode.clone(),
            _ => None,
        });
        let mut spec = self.resolve_launch_spec(&session.agent_id, &[]).await?;
        apply_opencode_mode(&mut spec, opencode_mode.as_deref(), &[]);

        // ネイティブ復元を試みる (復元できなかった場合の Replay 注入は呼び出し側)
        let (agent_session_id, allow_fresh) = match mode {
            // `--replay` 指定時はエージェント側IDを渡さず新規作成させる
            ResumeMode::AllowReplay { force_replay: true } => (None, true),
            ResumeMode::AllowReplay {
                force_replay: false,
            } => (session.agent_session_id.clone(), true),
            ResumeMode::NativeOnly => (session.agent_session_id.clone(), false),
        };
        let started = self
            .start_driver_session(StartDriverParams {
                session_id,
                title: &session.title,
                cwd: &cwd,
                spec: &spec,
                extra_args: Vec::new(),
                initial_mode: None,
                has_custom_title: true,
                resume: Some(ResumeRequest {
                    agent_session_id,
                    allow_fresh,
                }),
            })
            .await?;

        // 再開の事実を状態投影へ反映する (stopped / error → idle)
        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Idle,
                    error_message: None,
                },
            )
            .await?;

        Ok((started, events))
    }

    /// 停止済みセッションをネイティブ復元で自動再開する (プロンプト送信前)。
    ///
    /// 明示 resume と同じ `resuming` ガードで直列化し、他の再開処理が進行中の
    /// 場合は完了を待ってから制御を返す。ネイティブ復元できない場合は
    /// [`NodeError::ResumeRequired`] を返し、セッションは停止状態のまま残す
    /// (履歴 Replay は暗黙実行せず、クライアントに明示的な Resume を促す)。
    async fn auto_resume_for_prompt(&self, session_id: &str) -> Result<(), NodeError> {
        let deadline = Instant::now() + AUTO_RESUME_TIMEOUT;
        loop {
            // ロックは await を跨がないよう、ガードの取得と判定をスコープで閉じる
            let acquired = {
                let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
                if sessions.active.contains_key(session_id) {
                    return Ok(());
                }
                sessions.resuming.insert(session_id.to_owned())
            };
            if acquired {
                let result = self
                    .resume_stopped_session(session_id, ResumeMode::NativeOnly)
                    .await
                    .map(|(started, _events)| {
                        // ドライバ契約: ネイティブ限定 (`allow_fresh = false`) では
                        // 復元できない場合に起動失敗 (Err) となるため、ここでは
                        // `context_restored = true` のみが正常である
                        if !started.context_restored {
                            tracing::warn!(
                                session_id,
                                "native-only resume returned a fresh session"
                            );
                        }
                    });
                self.inner
                    .sessions
                    .lock()
                    .expect("sessions poisoned")
                    .resuming
                    .remove(session_id);
                return result;
            }
            if Instant::now() >= deadline {
                return Err(NodeError::InvalidState(format!(
                    "session is resuming: {session_id}"
                )));
            }
            tokio::time::sleep(RESUME_WAIT_INTERVAL).await;
        }
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
    ///
    /// セッションが停止済み (非 active) の場合は、ネイティブ復元での自動再開を
    /// 試みてから送信する。ネイティブ復元に非対応のエージェントでは
    /// [`NodeError::ResumeRequired`] を返すため、クライアントは明示的な Resume
    /// (履歴 Replay) を案内する。
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

        let active = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            sessions.active.contains_key(session_id)
        };
        if !active {
            // 停止済みセッションは自動再開 (ネイティブ復元のみ) してから送信する
            self.auto_resume_for_prompt(session_id).await?;
        }

        // busy チェックと予約 (check-and-set)
        {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get_mut(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
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
            let session = sessions.active.get_mut(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
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

    /// elicitation (構造化入力) へ応答する (2 回目以降は `AlreadyResolved`)。
    pub async fn respond_elicitation(
        &self,
        command_id: &str,
        session_id: &str,
        elicitation_id: &str,
        action: ElicitationAction,
        content: serde_json::Value,
        resolved_by: &str,
    ) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }

        let (handle, summary) = {
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get_mut(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            let Some(summary) = session.pending_elicitations.remove(elicitation_id) else {
                return Err(NodeError::AlreadyResolved(elicitation_id.to_owned()));
            };
            (Arc::clone(&session.handle), summary)
        };
        if let Err(err) = handle
            .respond_elicitation(elicitation_id.to_owned(), action, content.clone())
            .await
        {
            // content の schema 違反等でドライバが受理しなかった場合は pending を
            // 戻す (ユーザーが修正して再送できるようにする)
            let mut sessions = self.inner.sessions.lock().expect("sessions poisoned");
            if let Some(session) = sessions.active.get_mut(session_id) {
                session
                    .pending_elicitations
                    .insert(elicitation_id.to_owned(), summary);
            }
            return Err(NodeError::Server(format!("failed to respond: {err:#}")));
        }

        self.inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::ElicitationResolved {
                    elicitation_id: elicitation_id.to_owned(),
                    action,
                    content,
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
        let (handle, is_busy) = {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            let session = sessions.active.get(session_id).ok_or_else(|| {
                NodeError::InvalidSession(format!("session {session_id} is not active"))
            })?;
            (Arc::clone(&session.handle), session.busy)
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
            // 圧縮は1ターンとして実行されるため busy 中でも許可する
            // (opencode2 では次のステップ境界で実行される steer 配送)
            SessionControlAction::Compact => handle
                .compact_context()
                .await
                .map_err(|err| NodeError::Server(format!("failed to compact context: {err:#}"))),
            SessionControlAction::Cancel => {
                // すでにアイドル (ターン未実行) の場合はキャンセル対象がないため成功扱いとする
                if !is_busy {
                    return Ok(());
                }
                handle
                    .cancel_turn()
                    .await
                    .map_err(|err| NodeError::Server(format!("failed to cancel: {err:#}")))
            }
            SessionControlAction::Kill => handle
                .shutdown()
                .await
                .map_err(|err| NodeError::Server(format!("failed to shutdown: {err:#}"))),
        }
    }

    /// セッションをアーカイブ/復元する (`fxg session archive` / Web UI)。
    ///
    /// アーカイブは可逆な可視性フラグで、稼働状態は変更しない。
    /// 戻り値はアーカイブ日時 (Unix epoch ms。復元時は `None`)。
    pub async fn set_archived(
        &self,
        command_id: &str,
        session_id: &str,
        archived: bool,
    ) -> Result<Option<i64>, NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }
        self.ensure_session_exists(session_id).await?;
        let envelope = self
            .inner
            .bus
            .record(
                session_id,
                UnifiedEventPayload::SessionArchived { archived },
            )
            .await?;
        Ok(archived.then_some(envelope.created_at))
    }

    /// セッションを削除する (`fxg session delete` / Web UI)。
    ///
    /// 稼働中 (エージェントプロセス生存) の場合は停止を待ってから
    /// `SessionDeleted` (tombstone) を追記する。本イベントより前のイベント本文は
    /// 同一トランザクションでパージされる (復元不能)。
    pub async fn delete(&self, command_id: &str, session_id: &str) -> Result<(), NodeError> {
        if !self.begin_command(command_id) {
            return Err(NodeError::CommandDuplicate(command_id.to_owned()));
        }
        self.ensure_session_exists(session_id).await?;
        {
            let sessions = self.inner.sessions.lock().expect("sessions poisoned");
            if sessions.resuming.contains(session_id) {
                return Err(NodeError::InvalidState(format!(
                    "session is resuming: {session_id}"
                )));
            }
        }
        self.stop_if_active(session_id).await?;
        self.inner
            .bus
            .record(session_id, UnifiedEventPayload::SessionDeleted {})
            .await?;
        Ok(())
    }

    /// セッションが稼働中 (エージェントプロセス生存) か。
    pub fn is_active(&self, session_id: &str) -> bool {
        self.inner
            .sessions
            .lock()
            .expect("sessions poisoned")
            .active
            .contains_key(session_id)
    }

    /// セッションが存在する (削除済みでない) ことを検証する。
    async fn ensure_session_exists(&self, session_id: &str) -> Result<(), NodeError> {
        if self.inner.bus.db().get_session(session_id).await?.is_none() {
            return Err(NodeError::InvalidSession(session_id.to_owned()));
        }
        Ok(())
    }

    /// 稼働中セッションを停止し、エージェントプロセスの終了を待つ。
    ///
    /// タイムアウト時は警告のみで続行する (削除を阻害しない。以降のイベントは
    /// 削除済みセッションへの追記として破棄される)。
    async fn stop_if_active(&self, session_id: &str) -> Result<(), NodeError> {
        if !self.is_active(session_id) {
            return Ok(());
        }
        let command_id = uuid_v7();
        match self
            .control(&command_id, session_id, &SessionControlAction::Kill)
            .await
        {
            // チェック後に自然終了していた場合はそれで良い
            Ok(()) | Err(NodeError::InvalidSession(_)) => {}
            Err(err) => return Err(err),
        }
        let deadline = Instant::now() + STOP_BEFORE_DELETE_TIMEOUT;
        while self.is_active(session_id) {
            if Instant::now() >= deadline {
                tracing::warn!(
                    session_id,
                    "timed out waiting for session to stop before delete"
                );
                break;
            }
            tokio::time::sleep(RESUME_WAIT_INTERVAL).await;
        }
        Ok(())
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

    /// 回答待ち elicitation 一覧 (`fxg inbox list`)。
    pub fn pending_elicitations(&self) -> Vec<PendingElicitationSummary> {
        let sessions = self.inner.sessions.lock().expect("sessions poisoned");
        let mut list: Vec<PendingElicitationSummary> = sessions
            .active
            .values()
            .flat_map(|session| session.pending_elicitations.values().cloned())
            .collect();
        list.sort_by(|a, b| a.elicitation_id.cmp(&b.elicitation_id));
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
            // 温存せず完全に破棄する (`shutdown` はプールへ返却するため使わない)
            if let Err(err) = handle.dispose().await {
                tracing::warn!(session_id = %session_id, "failed to shutdown: {err:#}");
            }
            remove_snapshot_index(&self.inner.paths, &session_id);
            stopped.push(session_id);
        }
        // アイドル (warm) プロセスも破棄してプールを空にする
        let drivers: Vec<Arc<dyn AgentDriver>> =
            { self.inner.drivers.lock().expect("drivers poisoned").clone() };
        for driver in drivers {
            let disposed = driver.shutdown_idle().await;
            if disposed > 0 {
                tracing::info!(
                    driver = driver.driver_kind(),
                    count = disposed,
                    "disposed idle agent processes"
                );
            }
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
                    UnifiedEventPayload::ElicitationRequest {
                        elicitation_id,
                        message,
                        mode,
                        requested_schema,
                        ..
                    } => {
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        if let Some(session) = sessions.active.get_mut(&session_id) {
                            session.pending_elicitations.insert(
                                elicitation_id.clone(),
                                PendingElicitationSummary {
                                    session_id: session_id.clone(),
                                    elicitation_id: elicitation_id.clone(),
                                    message: message.clone(),
                                    mode: mode.clone(),
                                    requested_schema: requested_schema.clone(),
                                },
                            );
                        }
                    }
                    UnifiedEventPayload::ElicitationResolved { elicitation_id, .. } => {
                        // ドライバ側 (ターンキャンセル時の自動解決等) で解決された
                        // elicitation もここに来る。fxg 経由の応答は
                        // `SessionManager::respond_elicitation` が記録済みのため、
                        // 回答待ち一覧に残っている場合のみ記録する。
                        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
                        let already_recorded =
                            sessions.active.get_mut(&session_id).is_some_and(|session| {
                                session
                                    .pending_elicitations
                                    .remove(elicitation_id)
                                    .is_none()
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
                | SessionStatus::WaitingInput
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
    {
        let mut sessions = inner.sessions.lock().expect("sessions poisoned");
        sessions.active.remove(&session_id);
    }
    remove_snapshot_index(&inner.paths, &session_id);
}

/// セッションのシャドウ Git インデックスファイルを削除する (リソース解放)。
fn remove_snapshot_index(paths: &NodePaths, session_id: &str) {
    let path = paths.snapshot_index_path(session_id);
    if let Err(err) = std::fs::remove_file(&path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(session_id, path = %path.display(), "failed to remove snapshot index: {err:#}");
    }
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
            SessionStatus::Running | SessionStatus::WaitingPermission | SessionStatus::WaitingInput
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

/// 履歴 Replay を注入する目的 (プロンプト冒頭の説明文に使う)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayPurpose {
    /// 会話の分岐 (`fxg session fork`)
    Fork,
    /// 停止セッションの再開 (`fxg session resume`)
    Resume,
}

impl ReplayPurpose {
    /// プロンプト冒頭の説明文を組み立てる。
    fn header(self, range: &str) -> String {
        match self {
            Self::Fork => format!(
                "以下の履歴は、以前のセッション{range}をこの時点から \
                 Fork したものです。この文脈を引き継いで作業を続けてください。\n\n"
            ),
            Self::Resume => format!(
                "以下の履歴は、停止していたセッション{range}を再開したものです。 \
                 この文脈を引き継いで作業を続けてください。\n\n"
            ),
        }
    }
}

/// 履歴 Replay (fork / resume) 用の会話コンテキストを組み立てる。
///
/// 会話 (ユーザー / エージェントメッセージ) と、ツール呼び出しの要約・
/// 変更ファイルを `node_seq` 順に連結する (`up_to_seq` で打ち切る)。
/// 会話が無い (履歴なし) 場合は `None`。
fn build_history_replay_context(
    events: &[SessionEventEnvelope],
    up_to_seq: u64,
    purpose: ReplayPurpose,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for event in events {
        if event.node_seq > up_to_seq {
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
    assemble_replay_context(parts, up_to_seq, purpose)
}

/// 中央サーバーから届いた構造化履歴 ([`ForkHistoryItem`]) から
/// Replay 注入用のコンテキストを組み立てる (別ノード・一時VMへの引き継ぎ)。
fn build_history_replay_context_from_items(
    items: &[ForkHistoryItem],
    purpose: ReplayPurpose,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for item in items {
        let role = if item.role == "user" {
            "User"
        } else {
            "Assistant"
        };
        if !item.text.trim().is_empty() {
            parts.push(format!("### {role}\n{}", item.text));
        }
        if let Some(summary) = item.tool_summary.as_deref()
            && !summary.trim().is_empty()
        {
            parts.push(format!("- {summary}"));
        }
    }
    assemble_replay_context(parts, 0, purpose)
}

/// Replay コンテキストの共通組み立て (上限文字数で打ち切る)。
fn assemble_replay_context(
    parts: Vec<String>,
    up_to_seq: u64,
    purpose: ReplayPurpose,
) -> Option<String> {
    /// 注入するコンテキストの最大文字数 (過大なプロンプトを防ぐ)。
    const MAX_CONTEXT_CHARS: usize = 16_000;

    if parts.is_empty() {
        return None;
    }
    let range = if up_to_seq > 0 {
        format!(" (node_seq <= {up_to_seq})")
    } else {
        String::new()
    };
    let mut context = purpose.header(&range);
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
    async fn cancel_on_idle_session_is_safe_noop() {
        let (manager, _mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        let res = manager
            .control("c2", &outcome.session_id, &SessionControlAction::Cancel)
            .await;
        assert!(res.is_ok());
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
    async fn elicitation_resolution_is_idempotent() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        // ドライバから elicitation (質問) を注入
        mock.emit(DriverEvent::Event(
            UnifiedEventPayload::ElicitationRequest {
                elicitation_id: "elic-1".to_owned(),
                message: "どの戦略で進めますか?".to_owned(),
                mode: "form".to_owned(),
                requested_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "strategy": { "type": "string", "enum": ["a", "b"] } },
                    "required": ["strategy"]
                }),
                tool_call_id: None,
            },
        ));
        for _ in 0..100 {
            if !manager.pending_elicitations().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(manager.pending_elicitations().len(), 1);
        assert_eq!(
            manager.pending_elicitations()[0].message,
            "どの戦略で進めますか?"
        );

        manager
            .respond_elicitation(
                "c2",
                &outcome.session_id,
                "elic-1",
                ElicitationAction::Accept,
                serde_json::json!({ "strategy": "a" }),
                "cli",
            )
            .await
            .expect("resolve");
        assert!(manager.pending_elicitations().is_empty());
        let recorded = mock.elicitations();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "elic-1");
        assert_eq!(recorded[0].1, ElicitationAction::Accept);
        assert_eq!(recorded[0].2["strategy"], "a");

        // 2 回目は ALREADY_RESOLVED
        let err = manager
            .respond_elicitation(
                "c3",
                &outcome.session_id,
                "elic-1",
                ElicitationAction::Decline,
                serde_json::Value::Null,
                "cli",
            )
            .await
            .expect_err("already resolved");
        assert!(matches!(err, NodeError::AlreadyResolved(_)));
    }

    #[tokio::test]
    async fn driver_resolved_elicitation_clears_pending() {
        let (manager, mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c1", dir.path(), "mock", &[], None, false)
            .await
            .expect("ensure");
        wait_idle(&manager, &outcome.session_id).await;

        // ドライバが発行した elicitation が pending に載る
        mock.emit(DriverEvent::Event(
            UnifiedEventPayload::ElicitationRequest {
                elicitation_id: "elic-2".to_owned(),
                message: "続行しますか?".to_owned(),
                mode: "form".to_owned(),
                requested_schema: serde_json::json!({ "type": "object", "properties": {} }),
                tool_call_id: None,
            },
        ));
        for _ in 0..100 {
            if !manager.pending_elicitations().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(manager.pending_elicitations().len(), 1);

        // ドライバ側の自動解決 (ターンキャンセル等) で pending が消える
        mock.emit(DriverEvent::Event(
            UnifiedEventPayload::ElicitationResolved {
                elicitation_id: "elic-2".to_owned(),
                action: ElicitationAction::Cancel,
                content: serde_json::Value::Null,
                resolved_by: "system".to_owned(),
            },
        ));
        for _ in 0..100 {
            if manager.pending_elicitations().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(manager.pending_elicitations().is_empty());
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

    /// セッションが `stopped` になり active 一覧から外れるまで待つ。
    async fn wait_stopped(manager: &SessionManager, session_id: &str) {
        for _ in 0..200 {
            let row = manager
                .bus()
                .db()
                .get_session(session_id)
                .await
                .expect("query");
            if row.is_some_and(|session| session.status == SessionStatus::Stopped) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("session did not stop");
    }

    /// 会話 1 ターン + エージェント側ID付きの停止済みセッションを用意する。
    ///
    /// 戻り値は (リポジトリパス, セッションID)。
    async fn setup_stopped_session(
        manager: &SessionManager,
        mock: &MockAgent,
        dir: &tempfile::TempDir,
        prompt: &str,
    ) -> (PathBuf, String) {
        let (repo, session_id) = setup_repo_session(manager, dir).await;
        manager
            .send_prompt("c2", &session_id, prompt, "cli")
            .await
            .expect("prompt");
        wait_user_message(manager, &session_id, prompt).await;
        mock.emit(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
            agent_session_id: "agent-sess-1".to_owned(),
        }));
        wait_event_type(manager, &session_id, "session_agent_bound").await;
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(manager, &session_id).await;
        // ドライバ (エージェントプロセス) を終了させる
        mock.close_events();
        wait_stopped(manager, &session_id).await;
        (repo, session_id)
    }

    #[tokio::test]
    async fn resume_restores_natively_without_replay() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) =
            setup_stopped_session(&manager, &mock, &dir, "native resume").await;

        mock.set_resume_supported(true);
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c-resume",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect("resume");
        assert_eq!(outcome.session_id, session_id);
        assert!(outcome.context_restored);
        assert_eq!(outcome.attach_mode, AttachMode::AcpTui);

        // 再開後は status が idle に戻り、active 一覧へ復帰する
        wait_idle(&manager, &session_id).await;
        let row = manager
            .bus()
            .db()
            .get_session(&session_id)
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(row.status, SessionStatus::Idle);

        // ドライバへは記録済みのエージェント側IDで再開が要求される
        let start = mock.starts().last().cloned().expect("start_session");
        let resume = start.resume.expect("resume request");
        assert_eq!(resume.agent_session_id.as_deref(), Some("agent-sess-1"));
        assert!(resume.allow_fresh);

        // ネイティブ復元のため履歴 Replay は注入されない
        let batch = manager
            .bus()
            .db()
            .session_events_after(&session_id, 0, 200)
            .await
            .expect("events");
        assert!(
            !batch.events.iter().any(|event| matches!(
                &event.payload,
                UnifiedEventPayload::UserMessage { client_source, .. } if client_source == "resume"
            )),
            "native resume must not inject replay context"
        );
    }

    #[tokio::test]
    async fn resume_falls_back_to_replay_context() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) =
            setup_stopped_session(&manager, &mock, &dir, "fallback resume").await;

        // ネイティブ復元非対応のドライバ → Replay でコンテキストを継続する
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c-resume",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect("resume");
        assert!(!outcome.context_restored);

        // 履歴 Replay が `client_source = "resume"` の UserMessage として注入され、
        // エージェントへ送信される
        let batch = manager
            .bus()
            .db()
            .session_events_after(&session_id, 0, 200)
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
                } if client_source == "resume" => Some(text.clone()),
                _ => None,
            })
            .expect("injected resume context");
        assert!(injected.contains("fallback resume"), "履歴が含まれる");
        assert!(
            mock.prompts().iter().any(|prompt| prompt == &injected),
            "コンテキストがエージェントへ送信される"
        );
        // Replay ターンの完了 (Idle) で busy が解除される
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(&manager, &session_id).await;
    }

    #[tokio::test]
    async fn resume_force_replay_skips_agent_session_id() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) =
            setup_stopped_session(&manager, &mock, &dir, "force replay").await;

        // ネイティブ復元対応でも `--replay` はエージェント側IDを渡さない
        mock.set_resume_supported(true);
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c-resume",
                session_id: &session_id,
                force_replay: true,
            })
            .await
            .expect("resume");
        assert!(!outcome.context_restored);
        assert_eq!(
            mock.starts()
                .last()
                .and_then(|start| start.resume.as_ref())
                .and_then(|resume| resume.agent_session_id.clone()),
            None
        );
    }

    #[tokio::test]
    async fn resume_rejects_active_unknown_and_missing_workspace() {
        let (manager, mock, dir) = setup().await;

        // 未知のセッションは InvalidSession
        let err = manager
            .resume(ResumeParams {
                command_id: "u1",
                session_id: "missing-session",
                force_replay: false,
            })
            .await
            .expect_err("unknown session");
        assert!(matches!(err, NodeError::InvalidSession(_)));

        // 稼働中 (active) のセッションは InvalidState
        let (repo, session_id) = setup_repo_session(&manager, &dir).await;
        let err = manager
            .resume(ResumeParams {
                command_id: "c2",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect_err("active session");
        assert!(matches!(err, NodeError::InvalidState(_)));

        // 作業ディレクトリが消えている (Worktree 削除済み等) 場合は InvalidState
        mock.close_events();
        wait_stopped(&manager, &session_id).await;
        std::fs::remove_dir_all(&repo).expect("remove repo");
        let err = manager
            .resume(ResumeParams {
                command_id: "c3",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect_err("missing workspace");
        assert!(matches!(err, NodeError::InvalidState(_)));

        // 失敗しても排他ガード (`resuming`) は解除され、復旧後は再開できる
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        crate::testutil::init_test_repo(&repo).await;
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c4",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect("resume after recovery");
        assert_eq!(outcome.session_id, session_id);
    }

    #[tokio::test]
    async fn send_prompt_auto_resumes_stopped_session_natively() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "first turn").await;

        // ネイティブ復元対応エージェント: 明示 resume なしの送信で自動再開される
        mock.set_resume_supported(true);
        manager
            .send_prompt("c-next", &session_id, "auto resumed", "web")
            .await
            .expect("send prompt");

        // 記録済みエージェントセッションIDでネイティブ限定復元が要求される
        let start = mock.starts().last().cloned().expect("start_session");
        let resume = start.resume.expect("resume request");
        assert_eq!(resume.agent_session_id.as_deref(), Some("agent-sess-1"));
        assert!(!resume.allow_fresh, "自動レジュームはネイティブ限定");

        // 自動再開の完了で状態投影が idle に戻り、プロンプトが送信される
        wait_user_message(&manager, &session_id, "auto resumed").await;
        let row = manager
            .bus()
            .db()
            .get_session(&session_id)
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(row.status, SessionStatus::Idle);
        assert!(mock.prompts().iter().any(|prompt| prompt == "auto resumed"));

        // ネイティブ復元のため履歴 Replay は注入されない
        let batch = manager
            .bus()
            .db()
            .session_events_after(&session_id, 0, 200)
            .await
            .expect("events");
        assert!(
            !batch.events.iter().any(|event| matches!(
                &event.payload,
                UnifiedEventPayload::UserMessage { client_source, .. } if client_source == "resume"
            )),
            "native auto resume must not inject replay context"
        );

        // ターン完了で busy が解除される
        mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Idle,
            error_message: None,
        }));
        wait_not_busy(&manager, &session_id).await;
    }

    #[tokio::test]
    async fn send_prompt_requires_explicit_resume_when_native_unsupported() {
        let (manager, mock, dir) = setup().await;
        let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "first turn").await;

        // ネイティブ復元非対応: 送信は ResumeRequired で失敗し、履歴 Replay は
        // 暗黙実行されない (セッションは停止状態のまま)
        let err = manager
            .send_prompt("c-next", &session_id, "needs replay", "web")
            .await
            .expect_err("native resume is unavailable");
        assert!(matches!(err, NodeError::ResumeRequired(_)));
        assert_eq!(
            err.error_code(),
            fxg_protocol::common::ErrorCode::ResumeRequired
        );

        let row = manager
            .bus()
            .db()
            .get_session(&session_id)
            .await
            .expect("query")
            .expect("session row");
        assert_eq!(row.status, SessionStatus::Stopped, "停止状態のまま残る");
        assert!(manager.list_active().is_empty());
        assert!(
            !mock.prompts().iter().any(|prompt| prompt == "needs replay"),
            "プロンプトは送信されない"
        );

        // 明示的な resume (履歴 Replay) では再開できる
        let outcome = manager
            .resume(ResumeParams {
                command_id: "c-resume",
                session_id: &session_id,
                force_replay: false,
            })
            .await
            .expect("explicit resume");
        assert!(!outcome.context_restored);
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
    fn apply_opencode_mode_maps_bridge_and_acp() {
        fn opencode_spec() -> AgentLaunchSpec {
            AgentLaunchSpec {
                agent_id: OPENCODE2_ID.to_owned(),
                display_name: "OpenCode2".to_owned(),
                driver_kind: "opencode2".to_owned(),
                program: PathBuf::from("opencode"),
                args: vec!["serve".to_owned()],
                env: Vec::new(),
            }
        }

        // acp 指定: 引数に extra_args を含めて `opencode acp` に切り替える
        let mut spec = opencode_spec();
        assert_eq!(
            apply_opencode_mode(&mut spec, Some("acp"), &["--model".to_owned()]),
            Some("acp".to_owned())
        );
        assert_eq!(spec.driver_kind, "acp");
        assert_eq!(spec.args, vec!["acp", "--model"]);

        // bridge 指定: `opencode2 serve` に戻す
        assert_eq!(
            apply_opencode_mode(&mut spec, Some("bridge"), &[]),
            Some("bridge".to_owned())
        );
        assert_eq!(spec.driver_kind, "opencode2");
        assert_eq!(spec.args, vec!["serve"]);

        // 未指定: レジストリ既定を尊重し、実効モードのみ記録する
        assert_eq!(
            apply_opencode_mode(&mut spec, None, &[]),
            Some("bridge".to_owned())
        );
        assert_eq!(spec.driver_kind, "opencode2");

        // opencode2 以外は何もしない
        let mut other = opencode_spec();
        other.agent_id = "claude".to_owned();
        other.driver_kind = "acp".to_owned();
        assert_eq!(apply_opencode_mode(&mut other, Some("acp"), &[]), None);
        assert_eq!(other.driver_kind, "acp");
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
                mode: None,
                opencode_mode: None,
                extra_args: None,
                fork_context: None,
                restore_git_bundle: None,
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

    #[tokio::test]
    async fn shutdown_all_cleans_up_snapshot_index() {
        let (manager, _mock, dir) = setup().await;
        let outcome = manager
            .ensure_session("c_ensure", dir.path(), "mock", &[], None, false)
            .await
            .expect("ensure");

        // インデックスファイルを作成
        let index_path = manager.inner.paths.snapshot_index_path(&outcome.session_id);
        std::fs::create_dir_all(index_path.parent().unwrap()).expect("create parent dir");
        std::fs::write(&index_path, b"test index data").expect("write index");
        assert!(index_path.exists());

        // shutdown_all を実行
        let stopped = manager.shutdown_all().await;
        assert_eq!(stopped, vec![outcome.session_id]);

        // インデックスファイルが削除されていることを確認
        assert!(
            !index_path.exists(),
            "snapshot index must be cleaned up on shutdown"
        );
    }
}
