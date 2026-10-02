//! 中央サーバー (`fxg server`) の共有状態と Client API バックエンド実装。
//!
//! 設計: `docs/01-architecture-and-sync.md` §2.2・§3.1、`docs/03-protocol-and-api.md` §3。
//!
//! 中央サーバーはセッション状態の**投影** (`server.db`) のみを持ち、
//! 書き込み権限は実行ノードにある。クライアントからの操作は
//! [`NodeHub`] 経由で対象ノードへ中継し、結果を `command_id` 相関で返す。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use fxg_db::{Db, DbRole, PushSubscriptionRecord};
use fxg_protocol::client_api::{
    AgentOpResponse, AgentsResponse, ConnectionRole, CreateSessionRequest, CreateSessionResponse,
    CreateWorktreeRequest, IssueNodeTokenRequest, IssueNodeTokenResponse, KillSwitchResponse,
    NodeTokenSummary, NodeTokensResponse, ProjectLinkRequest, ProjectLinkResponse,
    ProjectScanRequest, ProjectScanResponse, ProvisionerSummary, ProvisionersResponse,
    PruneWorktreesRequest, PushSubscribeRequest, PushSubscribeResponse, RemoveWorktreeRequest,
    RespondElicitationRequest, RespondElicitationResponse, RespondPermissionRequest,
    RespondPermissionResponse, ResumeSessionRequest, ResumeSessionResponse,
    RotateAuthTokenResponse, SessionArchiveRequest, SessionArchiveResponse, SessionRevertRequest,
    SessionRevertResponse, WorktreeInfo, WorktreesResponse,
};
use fxg_protocol::common::{
    AgentAction, CommandResult, DiffScope, ErrorCode, ForkHistoryItem, WorkspaceDiffResponse,
    WorktreeAction,
};
use fxg_protocol::config::{GitCredentialConfig, ProvisionerConfig};
use fxg_protocol::events::UnifiedEventPayload;
use fxg_protocol::node_server::ServerToNodeMsg;
use fxg_protocol::util::uuid_v7;
use tokio::sync::broadcast;

use crate::api::{
    ApiError, ClientApiBackend, ClientCommand, ClientEvent, ClientInfo, PtyChannelError,
    PtyChannelEvent, PtySpawnParams, SystemExtras, auth,
};
use crate::error::ServerError;
use crate::hub::NodeHub;
use crate::provisioner::{self, ProvisionerManager, SpawnSessionRequest};
use crate::push::PushService;

/// Client WS / Node Hub 配信用ブロードキャスト容量。
const CLIENT_EVENT_CAPACITY: usize = 2048;

/// 中央サーバーの起動オプション。
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// データディレクトリ (`~/.flexagent`)
    pub fxg_home: PathBuf,
    /// HTTP/WS バインドアドレス (既定 `0.0.0.0:8080`)
    pub listen_addr: String,
    /// `localhost` / `127.0.0.1` 以外に許可する Host 一覧
    pub allowed_hosts: Vec<String>,
    /// 同一オリジン以外に許可する Origin 一覧
    pub allowed_origins: Vec<String>,
    /// 一時VMプロビジョナー定義 (`config.toml` の `[provisioners.*]`)
    pub provisioners: BTreeMap<String, ProvisionerConfig>,
    /// Git Credential Proxy 設定 (`config.toml` の `[server.git_credentials.*]`)
    pub git_credentials: BTreeMap<String, GitCredentialConfig>,
}

/// 中央サーバーの共有状態。
#[derive(Clone)]
pub struct ServerState {
    inner: Arc<ServerInner>,
}

struct ServerInner {
    db: Db,
    /// クライアント認証トークン (再生成時に差し替えるため `RwLock`)。
    auth_token: std::sync::RwLock<String>,
    hub: NodeHub,
    options: ServerOptions,
    client_events: broadcast::Sender<ClientEvent>,
    push: PushService,
    provisioners: ProvisionerManager,
}

impl std::fmt::Debug for ServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerState")
            .field("listen_addr", &self.inner.options.listen_addr)
            .finish()
    }
}

impl ServerState {
    /// `server.db` を開き、クライアント認証トークンを読み込んで状態を初期化する。
    pub async fn new(options: ServerOptions) -> Result<Self, ServerError> {
        std::fs::create_dir_all(&options.fxg_home)?;
        let db = Db::open(&fxg_db::hub_db_path(&options.fxg_home), DbRole::Hub).await?;
        let auth_token = auth::load_or_create_token(
            &options
                .fxg_home
                .join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
        )?;
        let (client_events, _) = broadcast::channel(CLIENT_EVENT_CAPACITY);
        let hub = NodeHub::new(db.clone(), client_events.clone());
        let push = PushService::load_or_create(&options.fxg_home)
            .map_err(|err| ServerError::Server(format!("web push init failed: {err}")))?;
        let provisioners = ProvisionerManager::new(
            options.fxg_home.clone(),
            options.provisioners.clone(),
            options.git_credentials.clone(),
        );
        Ok(Self {
            inner: Arc::new(ServerInner {
                db,
                auth_token: std::sync::RwLock::new(auth_token),
                hub,
                options,
                client_events,
                push,
                provisioners,
            }),
        })
    }

    /// 一時VMプロビジョナー管理。
    pub fn provisioners(&self) -> &ProvisionerManager {
        &self.inner.provisioners
    }

    /// `server.db`。
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// ノード接続レジストリ。
    pub fn hub(&self) -> &NodeHub {
        &self.inner.hub
    }

    /// VAPID Web Push 送信サービス。
    pub fn push(&self) -> &PushService {
        &self.inner.push
    }

    /// 起動オプション。
    pub fn options(&self) -> &ServerOptions {
        &self.inner.options
    }

    /// クライアント認証トークン。
    pub fn auth_token(&self) -> String {
        self.inner
            .auth_token
            .read()
            .expect("auth token poisoned")
            .clone()
    }

    /// クライアント認証トークンを再生成し、ディスクへ保存する。
    ///
    /// 旧トークン (Bearer / `fxg_session` Cookie) は即時無効化される。
    pub fn rotate_auth_token(&self) -> std::io::Result<String> {
        let token = auth::generate_token();
        auth::save_token(
            &self
                .inner
                .options
                .fxg_home
                .join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
            &token,
        )?;
        *self.inner.auth_token.write().expect("auth token poisoned") = token.clone();
        Ok(token)
    }

    /// 監査ログを `server.db.audit_logs` に記録する (失敗は警告のみ)。
    pub async fn record_audit(
        &self,
        action: &str,
        client: &ClientInfo,
        session_id: Option<&str>,
        details: serde_json::Value,
    ) {
        let mut audit =
            fxg_db::AuditLogRecord::new(action, client.ip.clone(), client.auth_subject.clone());
        audit.session_id = session_id.map(str::to_owned);
        audit.client_user_agent = client.user_agent.clone();
        audit.details = details;
        if let Err(err) = self.inner.db.append_audit_log(&audit).await {
            tracing::warn!("failed to record audit log ({action}): {err}");
        }
    }

    /// 対象セッションの実行ノードを解決する (`NODE_OFFLINE` はここで即時返却)。
    async fn session_node(&self, session_id: &str) -> Result<String, ApiError> {
        let session = self
            .inner
            .db
            .get_session(session_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found(format!("session not found: {session_id}")))?;
        if !self.inner.hub.is_online(&session.node_id).await {
            return Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node is offline: {}", session.node_id),
            ));
        }
        Ok(session.node_id)
    }

    /// ACP Registry カタログを提供できるオンラインノードを 1 台選ぶ。
    ///
    /// 一時VMより常駐ノードを優先し、最終疎通が新しい順に選択する。
    async fn select_agent_node(&self) -> Option<String> {
        let nodes = self.inner.db.list_nodes().await.ok()?;
        let mut online: Vec<_> = nodes.into_iter().filter(|node| node.is_online).collect();
        online.sort_by_key(|node| (node.is_ephemeral, -node.last_seen_at));
        online.first().map(|node| node.node_id.clone())
    }

    /// プロジェクトのこのノード上の既定実行ディレクトリを解決する。
    ///
    /// 直近使用した紐付け (Worktree 含む) を優先し、紐付けが無ければ
    /// 最新セッションの実行ディレクトリへフォールバックする
    /// ([`fxg_db::queries::resolve_default_local_path`])。
    async fn project_local_path(
        &self,
        project_id: &str,
        node_id: &str,
    ) -> Result<String, ApiError> {
        self.inner
            .db
            .resolve_default_local_path(project_id, node_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "no default directory for project {project_id} on node {node_id}; \
                     specify local_path or register the project (scan / link / worktree)"
                ))
            })
    }

    /// `always = true` の場合に `allow_always` 系の選択肢へ昇格した `option_id` を返す。
    async fn resolve_permission_option(
        &self,
        session_id: &str,
        request_id: &str,
        request: &RespondPermissionRequest,
    ) -> Result<String, ApiError> {
        if !request.always {
            return Ok(request.selected_option_id.clone());
        }
        let pending = self
            .inner
            .db
            .pending_permissions()
            .await
            .map_err(ApiError::from)?;
        if let Some(entry) = pending
            .iter()
            .find(|entry| entry.session_id == session_id && entry.request_id == request_id)
            && let Some(option) = entry.options.iter().find(|option| {
                option.kind.contains("always") || option.option_id.contains("always")
            })
        {
            return Ok(option.option_id.clone());
        }
        Ok(request.selected_option_id.clone())
    }
}

/// `CommandResult` を成功時 `session_id` 付きで組み立てるヘルパー。
fn command_failure(command_id: String, err: ApiError) -> CommandResult {
    CommandResult {
        command_id,
        success: false,
        code: Some(err.code),
        error: Some(err.message),
        session_id: None,
    }
}

impl ServerState {
    /// 論理プロジェクトの投影行を取得する。
    async fn project_record(
        &self,
        project_id: &str,
    ) -> Result<fxg_protocol::common::ProjectSummary, ApiError> {
        let projects = self
            .inner
            .db
            .list_projects()
            .await
            .map_err(ApiError::from)?;
        projects
            .into_iter()
            .find(|project| project.project_id == project_id)
            .ok_or_else(|| ApiError::not_found(format!("project not found: {project_id}")))
    }

    /// Context Fork 指定から、履歴 Replay と退避済み Git バンドルを解決する。
    ///
    /// - 履歴: 分岐元セッションの会話イベント (User / Agent / ToolCall) を
    ///   `from_node_seq` まで抽出し、`StartSession.fork_context_messages` に載せる
    /// - バンドル: `restore_git_bundle_b64` の明示指定がなければ、分岐元セッションの
    ///   `git_bundle_path` (一時VM破壊時に退避された bundle) を読み出して Base64 化する
    async fn prepare_fork(
        &self,
        request: &CreateSessionRequest,
    ) -> Result<(Option<Vec<ForkHistoryItem>>, Option<String>), ApiError> {
        let Some(fork) = &request.fork else {
            return Ok((None, None));
        };
        // 分岐元セッションの存在確認 (応答の相関・監査の前提)
        let source = self
            .inner
            .db
            .get_session(&fork.from_session_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| {
                ApiError::not_found(format!("session not found: {}", fork.from_session_id))
            })?;

        let batch = self
            .inner
            .db
            .session_events_after(&source.session_id, 0, 5_000)
            .await
            .map_err(ApiError::from)?;
        let mut items: Vec<ForkHistoryItem> = Vec::new();
        for event in batch.events {
            if let Some(limit) = fork.from_node_seq
                && event.node_seq > limit
            {
                break;
            }
            match event.payload {
                UnifiedEventPayload::UserMessage { text, .. } => items.push(ForkHistoryItem {
                    role: "user".to_owned(),
                    text,
                    tool_summary: None,
                }),
                UnifiedEventPayload::AgentMessage {
                    text,
                    is_complete: true,
                    ..
                } => items.push(ForkHistoryItem {
                    role: "agent".to_owned(),
                    text,
                    tool_summary: None,
                }),
                UnifiedEventPayload::ToolCall { title, status, .. } => {
                    items.push(ForkHistoryItem {
                        role: "tool".to_owned(),
                        text: String::new(),
                        tool_summary: Some(format!("[{status}] {title}")),
                    })
                }
                _ => {}
            }
        }
        let context = (!items.is_empty()).then_some(items);

        let bundle = match fork.restore_git_bundle_b64.clone() {
            Some(bundle) => Some(bundle),
            None => {
                match self
                    .inner
                    .db
                    .get_git_bundle_path(&fork.from_session_id)
                    .await
                {
                    Ok(Some(path)) => match std::fs::read(&path) {
                        Ok(bytes) => Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                        Err(err) => {
                            tracing::warn!(path, "failed to read stored git bundle: {err}");
                            None
                        }
                    },
                    Ok(None) => None,
                    Err(err) => {
                        tracing::debug!("failed to load git bundle path: {err}");
                        None
                    }
                }
            }
        };
        Ok((context, bundle))
    }
}

#[async_trait]
impl ClientApiBackend for ServerState {
    fn db(&self) -> &Db {
        &self.inner.db
    }

    fn auth_token(&self) -> String {
        ServerState::auth_token(self)
    }

    fn connection_role(&self) -> ConnectionRole {
        ConnectionRole::CentralServer
    }

    async fn system_extras(&self) -> Result<SystemExtras, ApiError> {
        Ok(SystemExtras {
            vapid_public_key: Some(self.inner.push.public_key().to_owned()),
            ..SystemExtras::default()
        })
    }

    fn subscribe_client_events(&self) -> broadcast::Receiver<ClientEvent> {
        self.inner.client_events.subscribe()
    }

    async fn dispatch_command(&self, command: ClientCommand) -> CommandResult {
        let command_id = command.command_id().to_owned();
        let (node_id, message) = match &command {
            ClientCommand::SendPrompt {
                session_id,
                text,
                client_source,
                ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::SendPrompt {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    text: text.clone(),
                    client_source: client_source.clone(),
                },
            ),
            ClientCommand::RespondPermission {
                session_id,
                request_id,
                selected_option_id,
                resolved_by,
                ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::RespondPermission {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    request_id: request_id.clone(),
                    selected_option_id: selected_option_id.clone(),
                    resolved_by: resolved_by.clone(),
                },
            ),
            ClientCommand::RespondElicitation {
                session_id,
                elicitation_id,
                action,
                content,
                resolved_by,
                ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::RespondElicitation {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    elicitation_id: elicitation_id.clone(),
                    action: *action,
                    content: content.clone(),
                    resolved_by: resolved_by.clone(),
                },
            ),
            ClientCommand::ControlSession {
                session_id, action, ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::ControlSession {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    action: action.clone(),
                },
            ),
        };

        match node_id {
            Ok(node_id) => self.run_ws_command(&node_id, &command_id, message).await,
            // ノードがオフライン・セッション不明の場合はキューイングせず即時返却
            Err(err) => command_failure(command_id, err),
        }
    }

    async fn create_session(
        &self,
        request: CreateSessionRequest,
        client: ClientInfo,
    ) -> Result<CreateSessionResponse, ApiError> {
        // Context Fork (別ノード・一時VMからの引き継ぎ): 履歴 Replay と
        // 退避済み Git バンドルを解決する
        let (fork_context, restore_bundle_b64) = self.prepare_fork(&request).await?;

        // 一時VMプロビジョナー: サーバーホスト上で子プロセスを起動する
        if let Some(provisioner_name) = request.provisioner.clone() {
            if request.node_id.is_some() {
                return Err(ApiError::bad_request(
                    "node_id and provisioner are mutually exclusive",
                ));
            }
            let project = self.project_record(&request.project_id).await?;
            let git_url = project.canonical_git_url.clone().ok_or_else(|| {
                ApiError::bad_request(format!(
                    "project {} has no canonical git url; cannot clone into an ephemeral node",
                    request.project_id
                ))
            })?;

            let session_id = uuid_v7();
            let node_id = format!("eph-{}", &session_id[..18.min(session_id.len())]);
            let git_branch = request
                .worktree
                .as_ref()
                .map(|spec| spec.branch.clone())
                .filter(|branch| !branch.trim().is_empty());
            let agent_id = request.agent_id.clone();

            // 一時ノードの事前登録 (sessions.node_id の FK / UI 表示)
            let idle_timeout_secs = self
                .provisioners()
                .config(&provisioner_name)?
                .idle_timeout_secs
                .unwrap_or(900);
            provisioner::register_ephemeral_node(
                self,
                &node_id,
                &provisioner_name,
                idle_timeout_secs,
            )
            .await?;

            // ブートストラップ中も UI にセッションを見せる仮投影行
            if let Err(err) = self
                .db()
                .upsert_provisional_session(&fxg_db::registration::ProvisionalSessionRecord {
                    session_id: session_id.clone(),
                    project_id: request.project_id.clone(),
                    project_name: project.name.clone(),
                    node_id: node_id.clone(),
                    local_path: provisioner::EPHEMERAL_WORKSPACE.to_owned(),
                    git_branch: git_branch.clone(),
                    agent_id: agent_id.clone(),
                    title: format!("{agent_id} @ {}", project.name),
                })
                .await
            {
                return Err(ApiError::from(err));
            }

            provisioner::spawn_session(
                self,
                SpawnSessionRequest {
                    session_id: session_id.clone(),
                    node_id: node_id.clone(),
                    provisioner: provisioner_name.clone(),
                    project_id: request.project_id.clone(),
                    git_url,
                    git_branch,
                    agent_id: agent_id.clone(),
                    initial_prompt: request.initial_prompt.clone(),
                    mode: request.mode.clone(),
                    opencode_mode: request.opencode_mode.clone(),
                    extra_args: request.extra_args.clone(),
                    fork_context,
                    restore_bundle_b64,
                },
            )
            .await?;

            self.record_audit(
                fxg_db::audit::actions::SESSION_START,
                &client,
                Some(&session_id),
                serde_json::json!({
                    "node_id": node_id,
                    "agent_id": agent_id,
                    "provisioner": provisioner_name,
                    "project_id": request.project_id,
                }),
            )
            .await;
            return Ok(CreateSessionResponse {
                session_id,
                command_id: request.command_id,
            });
        }

        let node_id = request
            .node_id
            .clone()
            .ok_or_else(|| ApiError::bad_request("node_id or provisioner is required"))?;
        if !self.inner.hub.is_online(&node_id).await {
            return Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node is offline: {node_id}"),
            ));
        }

        // Worktree を同時作成する場合はノード側で作成し、実パスを受け取る
        let local_path = if let Some(spec) = &request.worktree {
            let worktree_command_id = uuid_v7();
            let info = self
                .inner
                .hub
                .worktree_op(
                    &node_id,
                    &worktree_command_id,
                    ServerToNodeMsg::ManageWorktree {
                        command_id: worktree_command_id.clone(),
                        project_id: request.project_id.clone(),
                        action: WorktreeAction::Add {
                            branch: spec.branch.clone(),
                            base_branch: spec.base_branch.clone(),
                            new_path: spec.new_path.clone(),
                        },
                    },
                )
                .await?
                .ok_or_else(|| ApiError::internal("node returned no worktree info"))?;
            info.path
        } else if let Some(path) = request.local_path.clone() {
            path
        } else {
            self.project_local_path(&request.project_id, &node_id)
                .await?
        };

        let session_id = uuid_v7();
        let result = self
            .inner
            .hub
            .command(
                &node_id,
                &request.command_id,
                ServerToNodeMsg::StartSession {
                    command_id: request.command_id.clone(),
                    session_id: session_id.clone(),
                    project_id: request.project_id.clone(),
                    local_path: local_path.clone(),
                    agent_id: request.agent_id.clone(),
                    initial_prompt: request.initial_prompt.clone(),
                    mode: request.mode.clone(),
                    opencode_mode: request.opencode_mode.clone(),
                    extra_args: request.extra_args.clone(),
                    fork_context_messages: fork_context,
                    restore_git_bundle_b64: restore_bundle_b64,
                },
            )
            .await?;
        if !result.success {
            return Err(ApiError::from_code(
                result.code.unwrap_or(ErrorCode::Internal),
                result
                    .error
                    .unwrap_or_else(|| "start session failed".to_owned()),
            ));
        }

        self.record_audit(
            fxg_db::audit::actions::SESSION_START,
            &client,
            Some(&session_id),
            serde_json::json!({
                "node_id": node_id,
                "agent_id": request.agent_id,
                "local_path": local_path,
                "project_id": request.project_id,
            }),
        )
        .await;

        Ok(CreateSessionResponse {
            session_id,
            command_id: result.command_id,
        })
    }

    async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        request: RespondPermissionRequest,
        client: ClientInfo,
    ) -> Result<RespondPermissionResponse, ApiError> {
        let node_id = self.session_node(session_id).await?;
        let selected = self
            .resolve_permission_option(session_id, request_id, &request)
            .await?;
        let command_id = uuid_v7();
        let result = self
            .inner
            .hub
            .command(
                &node_id,
                &command_id,
                ServerToNodeMsg::RespondPermission {
                    command_id: command_id.clone(),
                    session_id: session_id.to_owned(),
                    request_id: request_id.to_owned(),
                    selected_option_id: selected.clone(),
                    resolved_by: request.resolved_by.clone(),
                },
            )
            .await?;

        if result.success {
            self.record_audit(
                fxg_db::audit::actions::PERMISSION_RESOLVED,
                &client,
                Some(session_id),
                serde_json::json!({
                    "request_id": request_id,
                    "selected_option_id": selected,
                    "resolved_by": request.resolved_by,
                    "node_id": node_id,
                }),
            )
            .await;
            Ok(RespondPermissionResponse {
                already_resolved: false,
            })
        } else if result.code == Some(ErrorCode::AlreadyResolved) {
            // 全クライアント横断の冪等解決 (2回目以降は正常遷移)
            Ok(RespondPermissionResponse {
                already_resolved: true,
            })
        } else {
            Err(ApiError::from_code(
                result.code.unwrap_or(ErrorCode::Internal),
                result.error.unwrap_or_else(|| "respond failed".to_owned()),
            ))
        }
    }

    async fn respond_elicitation(
        &self,
        session_id: &str,
        elicitation_id: &str,
        request: RespondElicitationRequest,
        client: ClientInfo,
    ) -> Result<RespondElicitationResponse, ApiError> {
        let node_id = self.session_node(session_id).await?;
        let command_id = uuid_v7();
        let result = self
            .inner
            .hub
            .command(
                &node_id,
                &command_id,
                ServerToNodeMsg::RespondElicitation {
                    command_id: command_id.clone(),
                    session_id: session_id.to_owned(),
                    elicitation_id: elicitation_id.to_owned(),
                    action: request.action,
                    content: request.content.clone(),
                    resolved_by: request.resolved_by.clone(),
                },
            )
            .await?;

        if result.success {
            self.record_audit(
                fxg_db::audit::actions::ELICITATION_RESOLVED,
                &client,
                Some(session_id),
                serde_json::json!({
                    "elicitation_id": elicitation_id,
                    "action": request.action.as_str(),
                    "resolved_by": request.resolved_by,
                    "node_id": node_id,
                }),
            )
            .await;
            Ok(RespondElicitationResponse {
                already_resolved: false,
            })
        } else if result.code == Some(ErrorCode::AlreadyResolved) {
            // 全クライアント横断の冪等解決 (2回目以降は正常遷移)
            Ok(RespondElicitationResponse {
                already_resolved: true,
            })
        } else {
            Err(ApiError::from_code(
                result.code.unwrap_or(ErrorCode::Internal),
                result.error.unwrap_or_else(|| "respond failed".to_owned()),
            ))
        }
    }

    async fn revert_session(
        &self,
        session_id: &str,
        request: SessionRevertRequest,
        client: ClientInfo,
    ) -> Result<SessionRevertResponse, ApiError> {
        let node_id = self.session_node(session_id).await?;
        let command_id = uuid_v7();
        let outcome = self
            .inner
            .hub
            .revert_session(
                &node_id,
                &command_id,
                ServerToNodeMsg::RevertSession {
                    command_id: command_id.clone(),
                    session_id: session_id.to_owned(),
                    target_node_seq: request.target_node_seq,
                },
            )
            .await?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_REVERT,
            &client,
            Some(session_id),
            serde_json::json!({
                "node_id": node_id,
                "target_node_seq": outcome.target_node_seq,
                "restored_tree_hash": outcome.restored_tree_hash,
                "restored_files": outcome.restored_files,
                "removed_files": outcome.removed_files,
            }),
        )
        .await;
        Ok(outcome)
    }

    async fn resume_session(
        &self,
        session_id: &str,
        request: ResumeSessionRequest,
        client: ClientInfo,
    ) -> Result<ResumeSessionResponse, ApiError> {
        let session = self
            .inner
            .db
            .get_session(session_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found(format!("session not found: {session_id}")))?;

        // 一時VM (provisioner) セッションの再開 (VM 再起動 + bundle 復元) は v2 のため
        // v1 では拒否する (常駐ノードのみ対象)。
        let is_ephemeral = self
            .inner
            .db
            .list_nodes()
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .any(|node| node.node_id == session.node_id && node.is_ephemeral);
        if is_ephemeral {
            return Err(ApiError::from_code(
                ErrorCode::InvalidState,
                "resume for ephemeral sessions is not supported yet",
            ));
        }

        // 実行ノードへ中継 (オフラインは `NODE_OFFLINE`)
        let node_id = self.session_node(session_id).await?;
        let command_id = uuid_v7();
        let context_restored = self
            .inner
            .hub
            .resume_session(
                &node_id,
                &command_id,
                ServerToNodeMsg::ResumeSession {
                    command_id: command_id.clone(),
                    session_id: session_id.to_owned(),
                    force_replay: request.force_replay,
                },
            )
            .await?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_RESUME,
            &client,
            Some(session_id),
            serde_json::json!({
                "node_id": node_id,
                "force_replay": request.force_replay,
                "context_restored": context_restored,
            }),
        )
        .await;
        Ok(ResumeSessionResponse {
            session_id: session_id.to_owned(),
            context_restored,
        })
    }

    async fn archive_session(
        &self,
        session_id: &str,
        request: SessionArchiveRequest,
        client: ClientInfo,
    ) -> Result<SessionArchiveResponse, ApiError> {
        // 実行ノードへ中継 (存在確認・オフラインは `session_node` が返す)
        let node_id = self.session_node(session_id).await?;
        let command_id = uuid_v7();
        let archived_at = self
            .inner
            .hub
            .archive_session(
                &node_id,
                &command_id,
                ServerToNodeMsg::ArchiveSession {
                    command_id: command_id.clone(),
                    session_id: session_id.to_owned(),
                    archived: request.archived,
                },
            )
            .await?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_ARCHIVE,
            &client,
            Some(session_id),
            serde_json::json!({
                "node_id": node_id,
                "archived": request.archived,
                "archived_at": archived_at,
            }),
        )
        .await;
        Ok(SessionArchiveResponse {
            session_id: session_id.to_owned(),
            archived_at,
        })
    }

    async fn delete_session(&self, session_id: &str, client: ClientInfo) -> Result<(), ApiError> {
        // 実行ノードへ中継 (存在確認・オフラインは `session_node` が返す)
        let node_id = self.session_node(session_id).await?;
        let command_id = uuid_v7();
        self.inner
            .hub
            .delete_session(
                &node_id,
                &command_id,
                ServerToNodeMsg::DeleteSession {
                    command_id: command_id.clone(),
                    session_id: session_id.to_owned(),
                },
            )
            .await?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_DELETE,
            &client,
            Some(session_id),
            serde_json::json!({ "node_id": node_id }),
        )
        .await;
        Ok(())
    }

    async fn prune_worktrees(
        &self,
        project_id: &str,
        request: PruneWorktreesRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError> {
        if !self.inner.hub.is_online(&request.node_id).await {
            return Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node is offline: {}", request.node_id),
            ));
        }
        let command_id = uuid_v7();
        self.inner
            .hub
            .worktree_op(
                &request.node_id,
                &command_id,
                ServerToNodeMsg::ManageWorktree {
                    command_id: command_id.clone(),
                    project_id: project_id.to_owned(),
                    action: WorktreeAction::Prune,
                },
            )
            .await?;
        self.record_audit(
            fxg_db::audit::actions::WORKTREE_PRUNE,
            &client,
            None,
            serde_json::json!({
                "node_id": request.node_id,
                "project_id": project_id,
            }),
        )
        .await;
        Ok(())
    }

    async fn scan_projects(
        &self,
        node_id: &str,
        request: ProjectScanRequest,
        client: ClientInfo,
    ) -> Result<ProjectScanResponse, ApiError> {
        if !self.inner.hub.is_online(node_id).await {
            return Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node is offline: {node_id}"),
            ));
        }
        let request_id = uuid_v7();
        let response = self
            .inner
            .hub
            .scan_projects(
                node_id,
                &request_id,
                ServerToNodeMsg::ProjectScan {
                    request_id: request_id.clone(),
                    dir: request.dir.clone(),
                },
            )
            .await?;
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &client,
            None,
            serde_json::json!({
                "action": "scan",
                "node_id": node_id,
                "dir": request.dir,
                "scanned_dirs": response.scanned_dirs,
            }),
        )
        .await;
        Ok(response)
    }

    async fn link_project(
        &self,
        node_id: &str,
        request: ProjectLinkRequest,
        client: ClientInfo,
    ) -> Result<ProjectLinkResponse, ApiError> {
        if !self.inner.hub.is_online(node_id).await {
            return Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node is offline: {node_id}"),
            ));
        }
        let request_id = uuid_v7();
        let response = self
            .inner
            .hub
            .link_project(
                node_id,
                &request_id,
                ServerToNodeMsg::ProjectLink {
                    request_id: request_id.clone(),
                    project_id: request.project_id.clone(),
                    local_path: request.local_path.clone(),
                },
            )
            .await?;
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &client,
            None,
            serde_json::json!({
                "action": "link",
                "node_id": node_id,
                "project_id": response.project_id,
                "local_path": response.local_path,
            }),
        )
        .await;
        Ok(response)
    }

    async fn list_agents(&self) -> Result<AgentsResponse, ApiError> {
        let Some(node_id) = self.select_agent_node().await else {
            // カタログを提供できるオンラインノードが無い場合は空一覧を返す
            // (UI はノード一覧からオフライン理由を表示できる)
            return Ok(AgentsResponse { agents: Vec::new() });
        };
        let request_id = uuid_v7();
        let mut response = self
            .inner
            .hub
            .agents(
                &node_id,
                &request_id,
                ServerToNodeMsg::ListAgents {
                    request_id: request_id.clone(),
                },
            )
            .await?;

        // 全ノードの導入状態 (NodeHello の installed_agents) を統合する
        let nodes = self.inner.db.list_nodes().await.map_err(ApiError::from)?;
        for agent in &mut response.agents {
            agent.installed_nodes = nodes
                .iter()
                .filter(|node| node.installed_agents.iter().any(|id| id == &agent.id))
                .map(|node| node.node_id.clone())
                .collect();
            agent.installed = !agent.installed_nodes.is_empty();
        }
        Ok(response)
    }

    async fn manage_agent(
        &self,
        node_id: &str,
        action: AgentAction,
        client: ClientInfo,
    ) -> Result<AgentOpResponse, ApiError> {
        if !self.inner.hub.is_online(node_id).await {
            return Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node is offline: {node_id}"),
            ));
        }
        let request_id = uuid_v7();
        let message = self
            .inner
            .hub
            .manage_agent(
                node_id,
                &request_id,
                ServerToNodeMsg::ManageAgent {
                    request_id: request_id.clone(),
                    action: action.clone(),
                },
            )
            .await?;
        self.record_audit(
            fxg_db::audit::actions::AGENT_MANAGE,
            &client,
            None,
            serde_json::json!({
                "node_id": node_id,
                "action": action,
                "message": &message,
            }),
        )
        .await;
        Ok(AgentOpResponse { message })
    }

    async fn rotate_auth_token(
        &self,
        client: ClientInfo,
    ) -> Result<RotateAuthTokenResponse, ApiError> {
        let token = ServerState::rotate_auth_token(self).map_err(ApiError::internal)?;
        self.record_audit(
            fxg_db::audit::actions::AUTH_TOKEN_ROTATE,
            &client,
            None,
            serde_json::json!({ "scope": "server" }),
        )
        .await;
        Ok(RotateAuthTokenResponse { token })
    }

    async fn node_tokens(&self) -> Result<NodeTokensResponse, ApiError> {
        let tokens = self
            .inner
            .db
            .list_node_tokens()
            .await
            .map_err(ApiError::from)?;
        Ok(NodeTokensResponse {
            tokens: tokens
                .into_iter()
                .map(|(node_id, hash)| NodeTokenSummary {
                    node_id,
                    token_prefix: hash.chars().take(12).collect(),
                })
                .collect(),
        })
    }

    async fn issue_node_token(
        &self,
        request: IssueNodeTokenRequest,
        client: ClientInfo,
    ) -> Result<IssueNodeTokenResponse, ApiError> {
        let node_id = request.node_id.trim().to_owned();
        if node_id.is_empty() {
            return Err(ApiError::bad_request("node_id must not be empty"));
        }
        // 未登録のノードはプレースホルダで登録する
        // (ノード接続時の `NodeHello` で実情報に上書きされる)
        let nodes = self.inner.db.list_nodes().await.map_err(ApiError::from)?;
        if !nodes.iter().any(|node| node.node_id == node_id) {
            self.inner
                .db
                .upsert_node(&fxg_db::NodeRecord::new(
                    node_id.clone(),
                    node_id.clone(),
                    "unknown",
                    "unknown",
                    "unknown",
                ))
                .await
                .map_err(ApiError::from)?;
        }
        let token = auth::generate_token();
        self.inner
            .db
            .set_node_token_hash(&node_id, Some(&auth::hash_token(&token)))
            .await
            .map_err(ApiError::from)?;
        self.record_audit(
            fxg_db::audit::actions::NODE_TOKEN_MANAGE,
            &client,
            None,
            serde_json::json!({ "action": "issue", "node_id": &node_id }),
        )
        .await;
        Ok(IssueNodeTokenResponse { node_id, token })
    }

    async fn revoke_node_token(&self, node_id: &str, client: ClientInfo) -> Result<(), ApiError> {
        self.inner
            .db
            .set_node_token_hash(node_id, None)
            .await
            .map_err(ApiError::from)?;
        self.record_audit(
            fxg_db::audit::actions::NODE_TOKEN_MANAGE,
            &client,
            None,
            serde_json::json!({ "action": "revoke", "node_id": node_id }),
        )
        .await;
        Ok(())
    }

    async fn list_worktrees(&self, project_id: &str) -> Result<WorktreesResponse, ApiError> {
        let projects = self
            .inner
            .db
            .list_projects()
            .await
            .map_err(ApiError::from)?;
        let project = projects
            .iter()
            .find(|project| project.project_id == project_id)
            .ok_or_else(|| ApiError::not_found(format!("project not found: {project_id}")))?;
        let worktrees = project
            .bindings
            .iter()
            .map(|binding| WorktreeInfo {
                node_id: binding.node_id.clone(),
                path: binding.local_path.clone(),
                branch: binding.git_branch.clone(),
                // HEAD コミットはノードのリアルタイム報告 (NodeHello) には含まれない
                head_commit: None,
                is_main: !binding.is_worktree,
            })
            .collect();
        Ok(WorktreesResponse { worktrees })
    }

    async fn create_worktree(
        &self,
        project_id: &str,
        request: CreateWorktreeRequest,
        _client: ClientInfo,
    ) -> Result<WorktreeInfo, ApiError> {
        if !self.inner.hub.is_online(&request.node_id).await {
            return Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node is offline: {}", request.node_id),
            ));
        }
        let command_id = uuid_v7();
        let info = self
            .inner
            .hub
            .worktree_op(
                &request.node_id,
                &command_id,
                ServerToNodeMsg::ManageWorktree {
                    command_id: command_id.clone(),
                    project_id: project_id.to_owned(),
                    action: WorktreeAction::Add {
                        branch: request.branch,
                        base_branch: request.base_branch,
                        new_path: request.path,
                    },
                },
            )
            .await?
            .ok_or_else(|| ApiError::internal("node returned no worktree info"))?;
        Ok(info)
    }

    async fn remove_worktree(
        &self,
        project_id: &str,
        request: RemoveWorktreeRequest,
        _client: ClientInfo,
    ) -> Result<(), ApiError> {
        if !self.inner.hub.is_online(&request.node_id).await {
            return Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node is offline: {}", request.node_id),
            ));
        }
        let command_id = uuid_v7();
        self.inner
            .hub
            .worktree_op(
                &request.node_id,
                &command_id,
                ServerToNodeMsg::ManageWorktree {
                    command_id: command_id.clone(),
                    project_id: project_id.to_owned(),
                    action: WorktreeAction::Remove {
                        path: request.path,
                        force: request.force,
                    },
                },
            )
            .await?;
        Ok(())
    }

    async fn workspace_diff(
        &self,
        session_id: &str,
        scope: DiffScope,
        base_branch: Option<String>,
    ) -> Result<WorkspaceDiffResponse, ApiError> {
        let node_id = self.session_node(session_id).await?;
        let request_id = uuid_v7();
        self.inner
            .hub
            .git_diff(
                &node_id,
                &request_id,
                ServerToNodeMsg::GetGitDiff {
                    request_id: request_id.clone(),
                    session_id: session_id.to_owned(),
                    scope,
                    base_branch,
                },
            )
            .await
    }

    async fn browse_fs(
        &self,
        node_id: &str,
        path: Option<String>,
    ) -> Result<fxg_protocol::client_api::FsBrowseResponse, ApiError> {
        if !self.inner.hub.is_online(node_id).await {
            return Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node is offline: {node_id}"),
            ));
        }
        let request_id = uuid_v7();
        self.inner
            .hub
            .browse_fs(
                node_id,
                &request_id,
                ServerToNodeMsg::BrowseFs {
                    request_id: request_id.clone(),
                    path,
                },
            )
            .await
    }

    async fn provisioners(&self) -> Result<ProvisionersResponse, ApiError> {
        let provisioners = self
            .inner
            .options
            .provisioners
            .iter()
            .map(|(name, config)| ProvisionerSummary {
                name: name.clone(),
                description: config.description.clone(),
                idle_timeout_secs: config.idle_timeout_secs,
            })
            .collect();
        Ok(ProvisionersResponse { provisioners })
    }

    async fn provisioner_test(
        &self,
        name: &str,
    ) -> Result<fxg_protocol::client_api::ProvisionerTestResponse, ApiError> {
        crate::provisioner::provisioner_test(self, name).await
    }

    async fn kill_switch(
        &self,
        reason: &str,
        client: ClientInfo,
    ) -> Result<KillSwitchResponse, ApiError> {
        let notified = self
            .inner
            .hub
            .broadcast(ServerToNodeMsg::KillAllSessions {
                reason: reason.to_owned(),
            })
            .await;
        self.record_audit(
            fxg_db::audit::actions::KILL_SWITCH,
            &client,
            None,
            serde_json::json!({
                "reason": reason,
                "notified_nodes": notified,
            }),
        )
        .await;
        Ok(KillSwitchResponse {
            notified_nodes: notified.len() as u64,
        })
    }

    async fn push_subscribe(
        &self,
        request: PushSubscribeRequest,
        client: ClientInfo,
    ) -> Result<PushSubscribeResponse, ApiError> {
        if request.endpoint.trim().is_empty() {
            return Err(ApiError::bad_request("endpoint is required"));
        }
        let record = PushSubscriptionRecord {
            endpoint: request.endpoint.clone(),
            p256dh: request.p256dh.clone(),
            auth: request.auth.clone(),
            device_name: request.device_name.clone(),
        };
        fxg_db::push::upsert_push_subscription(self.inner.db.pool(), &record)
            .await
            .map_err(ApiError::from)?;
        tracing::info!(
            endpoint = %record.endpoint,
            device = record.device_name.as_deref().unwrap_or("unknown"),
            client_ip = %client.ip,
            "registered web push subscription"
        );
        Ok(PushSubscribeResponse { ok: true })
    }

    async fn pty_spawn(&self, params: PtySpawnParams) -> Result<(), PtyChannelError> {
        let session = self
            .inner
            .db
            .get_session(&params.session_id)
            .await
            .map_err(PtyChannelError::internal)?
            .ok_or_else(|| {
                PtyChannelError::not_found(format!("session not found: {}", params.session_id))
            })?;
        if !self.inner.hub.is_online(&session.node_id).await {
            return Err(PtyChannelError::offline(format!(
                "node is offline: {}",
                session.node_id
            )));
        }
        self.inner
            .hub
            .insert_pty(&params.pty_id, &session.node_id)
            .await;
        let result = self
            .inner
            .hub
            .send(
                &session.node_id,
                ServerToNodeMsg::PtySpawn {
                    pty_id: params.pty_id.clone(),
                    session_id: params.session_id.clone(),
                    cols: params.cols,
                    rows: params.rows,
                    shell_cmd: params.shell_cmd.clone(),
                },
            )
            .await;
        if let Err(err) = result {
            self.inner.hub.remove_pty(&params.pty_id).await;
            return Err(PtyChannelError::new(err.code.as_str(), err.message));
        }
        Ok(())
    }

    async fn pty_attach(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        if self.inner.hub.pty_subscribe(pty_id).await.is_some() {
            Ok(())
        } else {
            Err(PtyChannelError::not_found(format!(
                "pty session not found: {pty_id}"
            )))
        }
    }

    async fn pty_subscribe(
        &self,
        pty_id: &str,
    ) -> Result<broadcast::Receiver<PtyChannelEvent>, PtyChannelError> {
        self.inner
            .hub
            .pty_subscribe(pty_id)
            .await
            .ok_or_else(|| PtyChannelError::not_found(format!("pty session not found: {pty_id}")))
    }

    async fn pty_write(&self, pty_id: &str, data: &[u8]) -> Result<(), PtyChannelError> {
        use base64::Engine as _;
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(data);
        self.inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyInput {
                    pty_id: pty_id.to_owned(),
                    data_b64,
                },
            )
            .await
    }

    async fn pty_resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<(), PtyChannelError> {
        self.inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyResize {
                    pty_id: pty_id.to_owned(),
                    cols,
                    rows,
                },
            )
            .await
    }

    async fn pty_kill(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        self.inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyKill {
                    pty_id: pty_id.to_owned(),
                },
            )
            .await
    }

    async fn pty_disconnect(&self, pty_id: &str) {
        let _ = self
            .inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyKill {
                    pty_id: pty_id.to_owned(),
                },
            )
            .await;
        self.inner.hub.remove_pty(pty_id).await;
    }
}

impl ServerState {
    /// WS コマンドをノードへ中継し `CommandResult` を返す。
    async fn run_ws_command(
        &self,
        node_id: &str,
        command_id: &str,
        message: ServerToNodeMsg,
    ) -> CommandResult {
        match self.inner.hub.command(node_id, command_id, message).await {
            Ok(result) => result,
            Err(err) => command_failure(command_id.to_owned(), err),
        }
    }
}
