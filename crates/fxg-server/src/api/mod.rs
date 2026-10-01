//! クライアント向け共通 HTTP/WS API レイヤ。
//!
//! 中央サーバー (`fxg server`, `:8080`) とノードのローカルWebサーバー
//! (`fxg daemon`, `127.0.0.1:7860`) は**完全に同一の API パスと
//! WebSocket フォーマット**を提供する (設計: `docs/03-protocol-and-api.md` §3)。
//!
//! 本モジュールはその共通ルーター/ハンドラ実装であり、双方の差分
//! (DB の持ち主・コマンドの実行方法・ノード中継の有無) は
//! [`ClientApiBackend`] トレイトの実装に閉じ込める。
//!
//! - REST: [`client_router`] が構築する Axum ルーター
//! - Client WS (`/api/v1/client/ws`): [`ws`]
//! - PTY WS (`/api/v1/pty/ws`): [`pty`]
//! - セキュリティ (Host / Origin / Token 検証): [`security`]
//! - トークン生成・照合: [`auth`]

pub mod auth;
mod pty;
mod security;
mod static_ui;
mod ws;

use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use futures_util::SinkExt;
use fxg_db::{Db, DbError, SessionFilter};
use fxg_protocol::client_api::{
    AgentOpResponse, AgentsResponse, ApiErrorBody, ApiErrorResponse, AuditLogsResponse,
    AuthLoginRequest, ConnectionRole, CreateSessionRequest, CreateSessionResponse,
    CreateWorktreeRequest, InboxResponse, IssueNodeTokenRequest, IssueNodeTokenResponse,
    KillSwitchRequest, KillSwitchResponse, NodeTokensResponse, NodesResponse, ProjectLinkRequest,
    ProjectLinkResponse, ProjectScanRequest, ProjectScanResponse, ProjectsResponse,
    ProvisionersResponse, PruneWorktreesRequest, PushSubscribeRequest, PushSubscribeResponse,
    RemoveWorktreeRequest, RespondPermissionRequest, RespondPermissionResponse,
    ResumeSessionRequest, ResumeSessionResponse, RotateAuthTokenResponse, SearchResponse,
    ServerWsMessage, SessionListResponse, SessionRevertRequest, SessionRevertResponse,
    SystemInfoResponse, UpdateAgentsRequest, WorktreeInfo, WorktreesResponse,
};
use fxg_protocol::common::{
    AgentAction, CommandResult, DiffScope, ErrorCode, ProjectSummary, SessionControlAction,
    SessionStatus, WorkspaceDiffResponse,
};
use fxg_protocol::events::{SessionEventBatch, SessionEventEnvelope};
use tokio::sync::broadcast;

pub use pty::{PtyChannelError, PtyChannelEvent, PtySpawnParams};
pub use ws::REPLAY_BATCH_SIZE;

use self::auth::token_matches;

// ----------------------------------------------------------------------
// 共通型
// ----------------------------------------------------------------------

/// ロール非依存の API エラー (REST / WS の共通エラー形式へ変換される)。
#[derive(Debug, Clone)]
pub struct ApiError {
    /// HTTP ステータス
    pub status: StatusCode,
    /// 構造化エラーコード
    pub code: ErrorCode,
    /// 人間向けメッセージ
    pub message: String,
}

impl ApiError {
    /// 明示的なステータス・コードで生成する。
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// `ErrorCode` から推奨ステータスを決めて生成する。
    pub fn from_code(code: ErrorCode, message: impl Into<String>) -> Self {
        let status = match code {
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden | ErrorCode::PtyDisabled => StatusCode::FORBIDDEN,
            ErrorCode::NodeOffline => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::AlreadyResolved | ErrorCode::Busy | ErrorCode::CommandDuplicate => {
                StatusCode::CONFLICT
            }
            ErrorCode::InvalidState => StatusCode::BAD_REQUEST,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, code, message)
    }

    /// `400 INVALID_STATE`
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::InvalidState, message)
    }

    /// `404 NOT_FOUND`
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::NotFound, message)
    }

    /// `403 FORBIDDEN`
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::Forbidden, message)
    }

    /// `500 INTERNAL` (ログに残す)
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::warn!("api error: {err}");
        Self::from_code(ErrorCode::Internal, err.to_string())
    }
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match &err {
            DbError::SessionNotFound(_) | DbError::NodeNotFound(_) => {
                Self::not_found(err.to_string())
            }
            _ => Self::internal(err),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorResponse {
                error: ApiErrorBody {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

/// 接続クライアントの情報 (監査ログ用)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    /// 送信元 IP アドレス
    pub ip: String,
    /// User-Agent
    pub user_agent: Option<String>,
    /// 認証主体 (`bearer` / `cookie` / `local`)
    pub auth_subject: String,
}

impl ClientInfo {
    /// リクエストヘッダと接続元から組み立てる (ConnectInfo が無い場合は `local`)。
    pub fn from_request(headers: &HeaderMap, addr: Option<SocketAddr>) -> Self {
        let auth_subject = if headers.contains_key(header::AUTHORIZATION) {
            "bearer"
        } else if headers.contains_key(header::COOKIE) {
            "cookie"
        } else {
            "local"
        };
        Self {
            ip: addr
                .map(|addr| addr.ip().to_string())
                .unwrap_or_else(|| "local".to_owned()),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            auth_subject: auth_subject.to_owned(),
        }
    }

    /// `audit_logs.auth_subject` へ保存する値。
    pub fn audit_subject(&self) -> String {
        format!("{}:{}", self.auth_subject, self.ip)
    }
}

/// `GET /api/v1/system/info` のロール固有フィールド。
#[derive(Debug, Clone, Default)]
pub struct SystemExtras {
    /// Web Push 用 VAPID 公開鍵 (中央サーバーのみ)
    pub vapid_public_key: Option<String>,
    /// 未同期 Outbox イベント件数 (ローカルノード接続時のみ)
    pub unsynced_event_count: Option<u64>,
    /// 中央サーバーへの WebSocket 接続状態 (ローカルノード接続時のみ)
    pub central_connected: Option<bool>,
}

/// Client WS で配信されるライブイベント。
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// 永続化済みイベント (配信元ストアのカーソル確定済み)
    Persisted {
        /// イベント本体
        event: Box<SessionEventEnvelope>,
        /// `session_events.cursor` (差分再開カーソル)
        cursor: u64,
    },
    /// ストリーミング途中のエフェメラルチャンク (永続化されない)
    StreamDelta {
        /// 対象セッションID
        session_id: String,
        /// 差分本体
        delta: fxg_protocol::common::StreamDeltaPayload,
    },
    /// 一時VMブートストラップ (`stderr`) の進捗ログ (永続化されない)
    BootstrapLog {
        /// 対象セッションID
        session_id: String,
        /// ログ1行
        line: String,
    },
}

/// Client WS 経由のセッション操作コマンド (`Subscribe` / `Ping` を除く)。
#[derive(Debug, Clone, PartialEq)]
pub enum ClientCommand {
    /// プロンプト送信
    SendPrompt {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// プロンプト本文
        text: String,
        /// 送信元 (`cli` / `web` / `android`)
        client_source: String,
    },
    /// 承認リクエストへの回答
    RespondPermission {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// ACP request id
        request_id: String,
        /// 選択された `option_id`
        selected_option_id: String,
        /// 解決主体 (`cli` / `web` / `android_push`)
        resolved_by: String,
    },
    /// モード切替 / 設定変更 / キャンセル / Kill
    ControlSession {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// 実行する操作
        action: SessionControlAction,
    },
}

impl ClientCommand {
    /// WS メッセージからコマンドを取り出す (`Subscribe` / `Ping` は `None`)。
    pub fn from_message(message: &fxg_protocol::client_api::ClientWsMessage) -> Option<Self> {
        use fxg_protocol::client_api::ClientWsMessage;
        match message {
            ClientWsMessage::SendPrompt {
                command_id,
                session_id,
                text,
                client_source,
            } => Some(Self::SendPrompt {
                command_id: command_id.clone(),
                session_id: session_id.clone(),
                text: text.clone(),
                client_source: client_source.clone(),
            }),
            ClientWsMessage::RespondPermission {
                command_id,
                session_id,
                request_id,
                selected_option_id,
                resolved_by,
            } => Some(Self::RespondPermission {
                command_id: command_id.clone(),
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                selected_option_id: selected_option_id.clone(),
                resolved_by: resolved_by.clone(),
            }),
            ClientWsMessage::ControlSession {
                command_id,
                session_id,
                action,
            } => Some(Self::ControlSession {
                command_id: command_id.clone(),
                session_id: session_id.clone(),
                action: action.clone(),
            }),
            ClientWsMessage::Subscribe { .. } | ClientWsMessage::Ping => None,
        }
    }

    /// 相関ID。
    pub fn command_id(&self) -> &str {
        match self {
            Self::SendPrompt { command_id, .. }
            | Self::RespondPermission { command_id, .. }
            | Self::ControlSession { command_id, .. } => command_id,
        }
    }
}

// ----------------------------------------------------------------------
// バックエンドトレイト
// ----------------------------------------------------------------------

/// 共通ルーターが利用するバックエンド (ノード / 中央サーバー) 抽象。
///
/// DB 読み取り系のエンドポイントは [`Self::db`] だけで完結するため、
/// 実装側はロール固有の処理 (コマンド実行・中継) のみを提供する。
#[async_trait]
pub trait ClientApiBackend: Clone + Send + Sync + 'static {
    /// 接続先の DB (ローカルノードは `node.db`、中央サーバーは `server.db`)。
    fn db(&self) -> &Db;

    /// `GET /api/v1/projects` (論理プロジェクト一覧)。
    ///
    /// 既定実装は DB 投影をそのまま返す。ローカルノードは紐付けパスの
    /// 実在確認 (ライブ) を行った一覧を返すため上書きする。
    async fn list_projects(&self) -> Result<Vec<ProjectSummary>, ApiError> {
        self.db().list_projects().await.map_err(ApiError::from)
    }

    /// クライアント認証トークン。
    fn auth_token(&self) -> String;

    /// 接続先の種別 (`GET /api/v1/system/info` で返却)。
    fn connection_role(&self) -> ConnectionRole;

    /// fxg バージョン。
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// `GET /api/v1/system/info` のロール固有フィールド。
    async fn system_extras(&self) -> Result<SystemExtras, ApiError>;

    /// Client WS 用のライブイベントを購読する。
    fn subscribe_client_events(&self) -> broadcast::Receiver<ClientEvent>;

    /// Client WS 経由のコマンドを実行し、`CommandResult` を返す。
    async fn dispatch_command(&self, command: ClientCommand) -> CommandResult;

    /// `POST /api/v1/sessions` (新規セッション開始)。
    async fn create_session(
        &self,
        request: CreateSessionRequest,
        client: ClientInfo,
    ) -> Result<CreateSessionResponse, ApiError>;

    /// `POST /api/v1/sessions/:id/permissions/:req_id/respond` (冪等)。
    async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        request: RespondPermissionRequest,
        client: ClientInfo,
    ) -> Result<RespondPermissionResponse, ApiError>;

    /// `POST /api/v1/sessions/:id/revert` (Shadow Git Tree 巻き戻し)。
    async fn revert_session(
        &self,
        session_id: &str,
        request: SessionRevertRequest,
        client: ClientInfo,
    ) -> Result<SessionRevertResponse, ApiError>;

    /// `POST /api/v1/sessions/:id/resume` (停止済みセッションの再開)。
    async fn resume_session(
        &self,
        session_id: &str,
        request: ResumeSessionRequest,
        client: ClientInfo,
    ) -> Result<ResumeSessionResponse, ApiError>;

    /// `GET /api/v1/projects/:id/worktrees`
    async fn list_worktrees(&self, project_id: &str) -> Result<WorktreesResponse, ApiError>;

    /// `POST /api/v1/projects/:id/worktrees`
    async fn create_worktree(
        &self,
        project_id: &str,
        request: CreateWorktreeRequest,
        client: ClientInfo,
    ) -> Result<WorktreeInfo, ApiError>;

    /// `DELETE /api/v1/projects/:id/worktrees`
    async fn remove_worktree(
        &self,
        project_id: &str,
        request: RemoveWorktreeRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError>;

    /// `POST /api/v1/projects/:id/worktrees/prune`
    /// (削除済み Worktree 管理情報のクリーンアップ)。
    async fn prune_worktrees(
        &self,
        project_id: &str,
        request: PruneWorktreesRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError>;

    /// `POST /api/v1/nodes/:node_id/projects/scan`
    /// (指定ディレクトリ配下の Git リポジトリを一括スキャン・登録)。
    async fn scan_projects(
        &self,
        node_id: &str,
        request: ProjectScanRequest,
        client: ClientInfo,
    ) -> Result<ProjectScanResponse, ApiError>;

    /// `POST /api/v1/nodes/:node_id/projects/link`
    /// (任意ディレクトリを論理プロジェクトへ手動紐付け)。
    async fn link_project(
        &self,
        node_id: &str,
        request: ProjectLinkRequest,
        client: ClientInfo,
    ) -> Result<ProjectLinkResponse, ApiError>;

    /// `GET /api/v1/agents` (ACP Registry カタログ + ノード導入状態)。
    async fn list_agents(&self) -> Result<AgentsResponse, ApiError>;

    /// `POST /api/v1/nodes/:node_id/agents/...` (install / update / remove)。
    async fn manage_agent(
        &self,
        node_id: &str,
        action: AgentAction,
        client: ClientInfo,
    ) -> Result<AgentOpResponse, ApiError>;

    /// `POST /api/v1/auth/rotate-token` (クライアント認証トークン再生成)。
    ///
    /// 旧トークンは即時無効化されるため、応答の新トークンへ切り替えること。
    async fn rotate_auth_token(
        &self,
        client: ClientInfo,
    ) -> Result<RotateAuthTokenResponse, ApiError>;

    /// `GET /api/v1/nodes/tokens` (発行済みノード個別トークン一覧)。
    ///
    /// 中央サーバーのみ対応。ローカルノードの既定実装は `INVALID_STATE`
    /// を返す (ノードトークンはサーバー (`server.db`) の概念)。
    async fn node_tokens(&self) -> Result<NodeTokensResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "node tokens are only available on the central server",
        ))
    }

    /// `POST /api/v1/nodes/tokens` (新規ノードトークン発行。中央サーバーのみ)。
    async fn issue_node_token(
        &self,
        _request: IssueNodeTokenRequest,
        _client: ClientInfo,
    ) -> Result<IssueNodeTokenResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "node tokens are only available on the central server",
        ))
    }

    /// `DELETE /api/v1/nodes/tokens/:node_id` (トークン失効。中央サーバーのみ)。
    async fn revoke_node_token(&self, _node_id: &str, _client: ClientInfo) -> Result<(), ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "node tokens are only available on the central server",
        ))
    }

    /// `GET /api/v1/sessions/:id/diff`
    async fn workspace_diff(
        &self,
        session_id: &str,
        scope: DiffScope,
        base_branch: Option<String>,
    ) -> Result<WorkspaceDiffResponse, ApiError>;

    /// `GET /api/v1/nodes/:node_id/fs/browse`
    async fn browse_fs(
        &self,
        node_id: &str,
        path: Option<String>,
    ) -> Result<fxg_protocol::client_api::FsBrowseResponse, ApiError>;

    /// `GET /api/v1/provisioners`
    async fn provisioners(&self) -> Result<ProvisionersResponse, ApiError>;

    /// `POST /api/v1/provisioners/:name/test` (疎通検証)。
    ///
    /// 中央サーバーのみ対応。ローカルノードの既定実装は `INVALID_STATE`
    /// を返す (プロビジョナーはサーバーホスト上でのみ起動される)。
    async fn provisioner_test(
        &self,
        _name: &str,
    ) -> Result<fxg_protocol::client_api::ProvisionerTestResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "provisioners can only be tested on the central server",
        ))
    }

    /// `POST /api/v1/system/kill-switch`
    async fn kill_switch(
        &self,
        reason: &str,
        client: ClientInfo,
    ) -> Result<KillSwitchResponse, ApiError>;

    /// `POST /api/v1/push/subscribe` (Web Push 購読登録)。
    ///
    /// 中央サーバーのみ対応 (Phase 5)。ローカルノードの既定実装は
    /// `INVALID_STATE` を返す。
    async fn push_subscribe(
        &self,
        _request: PushSubscribeRequest,
        _client: ClientInfo,
    ) -> Result<PushSubscribeResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "web push is only available on the central server",
        ))
    }

    // ------------------------------------------------------------------
    // PTY WS (`/api/v1/pty/ws`)
    // ------------------------------------------------------------------

    /// PTY を起動する (`params.pty_id` は呼び出し側が採番)。
    async fn pty_spawn(&self, params: PtySpawnParams) -> Result<(), PtyChannelError>;

    /// 既存 PTY の存在確認 (アタッチ可否)。
    async fn pty_attach(&self, pty_id: &str) -> Result<(), PtyChannelError>;

    /// PTY 出力を購読する。
    async fn pty_subscribe(
        &self,
        pty_id: &str,
    ) -> Result<broadcast::Receiver<PtyChannelEvent>, PtyChannelError>;

    /// PTY へキー入力を書き込む。
    async fn pty_write(&self, pty_id: &str, data: &[u8]) -> Result<(), PtyChannelError>;

    /// PTY をリサイズする。
    async fn pty_resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<(), PtyChannelError>;

    /// PTY を終了する。
    async fn pty_kill(&self, pty_id: &str) -> Result<(), PtyChannelError>;

    /// PTY WS 切断時、このソケットで spawn した PTY を終了する (attach は対象外)。
    async fn pty_disconnect(&self, pty_id: &str);
}

// ----------------------------------------------------------------------
// ルーター
// ----------------------------------------------------------------------

/// ルーター構築オプション。
#[derive(Debug, Clone)]
pub struct ClientApiOptions {
    /// 実際にバインドされたポート (localhost / 127.0.0.1 の Host / Origin 検証用)
    pub port: u16,
    /// `localhost` 系以外に許可する Host 一覧
    /// (中央サーバーの `server.allowed_hosts`)
    pub extra_allowed_hosts: Vec<String>,
    /// 同一オリジン以外に許可する Origin 一覧
    /// (中央サーバーの `server.allowed_origins`)
    pub extra_allowed_origins: Vec<String>,
}

impl ClientApiOptions {
    /// ローカルノード用 (ループバックのみ許可)。
    pub fn local(port: u16) -> Self {
        Self {
            port,
            extra_allowed_hosts: Vec::new(),
            extra_allowed_origins: Vec::new(),
        }
    }
}

/// ルーターの共有状態。
#[derive(Clone)]
pub struct ClientApiState<B: ClientApiBackend> {
    /// バックエンド実装
    pub backend: B,
    /// セキュリティ設定
    pub options: Arc<ClientApiOptions>,
}

/// Client REST / WS / PTY WS の共通ルーターを構築する。
///
/// 呼び出し側は
/// `axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())`
/// で配信すること (クライアント IP を監査ログに記録するため)。
pub fn client_router<B: ClientApiBackend>(backend: B, options: ClientApiOptions) -> Router {
    let state = ClientApiState {
        backend,
        options: Arc::new(options),
    };
    let api = Router::new()
        .route("/api/v1/system/info", get(system_info::<B>))
        .route("/api/v1/system/kill-switch", post(kill_switch::<B>))
        .route("/api/v1/auth/login", post(auth_login::<B>))
        .route("/api/v1/auth/logout", post(auth_logout))
        .route("/api/v1/projects", get(list_projects::<B>))
        .route(
            "/api/v1/projects/{project_id}/worktrees",
            get(list_worktrees::<B>)
                .post(create_worktree::<B>)
                .delete(remove_worktree::<B>),
        )
        .route(
            "/api/v1/projects/{project_id}/worktrees/prune",
            post(prune_worktrees::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/projects/scan",
            post(scan_projects::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/projects/link",
            post(link_project::<B>),
        )
        .route("/api/v1/agents", get(list_agents::<B>))
        .route(
            "/api/v1/nodes/{node_id}/agents/{agent_id}/install",
            post(install_agent::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/agents/update",
            post(update_agents::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/agents/{agent_id}",
            delete(remove_agent::<B>),
        )
        .route(
            "/api/v1/nodes/tokens",
            get(node_tokens::<B>).post(issue_node_token::<B>),
        )
        .route(
            "/api/v1/nodes/tokens/{node_id}",
            delete(revoke_node_token::<B>),
        )
        .route("/api/v1/auth/rotate-token", post(rotate_auth_token::<B>))
        .route("/api/v1/nodes", get(list_nodes::<B>))
        .route("/api/v1/nodes/{node_id}/fs/browse", get(browse_fs::<B>))
        .route("/api/v1/provisioners", get(list_provisioners::<B>))
        .route(
            "/api/v1/provisioners/{provisioner}/test",
            post(test_provisioner::<B>),
        )
        .route(
            "/api/v1/sessions",
            get(list_sessions::<B>).post(create_session::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/events",
            get(session_events::<B>),
        )
        .route("/api/v1/sessions/{session_id}/diff", get(session_diff::<B>))
        .route(
            "/api/v1/sessions/{session_id}/permissions/{request_id}/respond",
            post(respond_permission::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/revert",
            post(revert_session::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/resume",
            post(resume_session::<B>),
        )
        .route("/api/v1/inbox", get(inbox::<B>))
        .route("/api/v1/search", post(search::<B>))
        .route("/api/v1/push/subscribe", post(push_subscribe::<B>))
        .route("/api/v1/audit/logs", get(audit_logs::<B>))
        .route("/api/v1/client/ws", get(client_ws::<B>))
        .route("/api/v1/pty/ws", get(pty_ws::<B>))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::security::<B>,
        ))
        .with_state(state);
    // API 以外の全パスは同梱した Web UI (SPA) を配信する。
    // セキュリティミドルウェアの外側に置く (UI アセット自体はトークン不要) 。
    api.fallback(get(static_ui::serve_ui))
}

// ----------------------------------------------------------------------
// REST ハンドラ
// ----------------------------------------------------------------------

async fn system_info<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    let extras = match state.backend.system_extras().await {
        Ok(extras) => extras,
        Err(err) => return err.into_response(),
    };
    Json(SystemInfoResponse {
        role: state.backend.connection_role(),
        version: state.backend.version().to_owned(),
        vapid_public_key: extras.vapid_public_key,
        unsynced_event_count: extras.unsynced_event_count,
        central_connected: extras.central_connected,
    })
    .into_response()
}

/// 認証 Cookie の名前 (`docs/03` §3.0)。
const SESSION_COOKIE: &str = "fxg_session";

/// `POST /api/v1/auth/login`: Web UI のトークン入力を検証し、`fxg_session`
/// Cookie (HttpOnly / SameSite=Strict) を発行する。
///
/// エンドポイント自体も共通セキュリティミドルウェアの認証を通過する必要が
/// あるため (`Authorization: Bearer`)、body のトークンは再検証のみを行う。
async fn auth_login<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Json(request): Json<AuthLoginRequest>,
) -> Response {
    let token = request.token.trim();
    if !token_matches(&state.backend.auth_token(), token) {
        return ApiError::from_code(ErrorCode::Unauthorized, "invalid auth token").into_response();
    }
    let cookie = format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

/// `POST /api/v1/auth/logout`: `fxg_session` Cookie を失効させる。
async fn auth_logout() -> Response {
    let cookie = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

async fn list_projects<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    match state.backend.list_projects().await {
        Ok(projects) => Json(ProjectsResponse { projects }).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn list_nodes<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    match state.backend.db().list_nodes().await {
        Ok(nodes) => Json(NodesResponse { nodes }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn browse_fs<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Query(query): Query<fxg_protocol::client_api::FsBrowseQuery>,
) -> Response {
    match state.backend.browse_fs(&node_id, query.path).await {
        Ok(resp) => Json(resp).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn list_provisioners<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.provisioners().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

/// `POST /api/v1/provisioners/:name/test`: プロビジョナーの起動とハンドシェイクを検証する。
async fn test_provisioner<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(provisioner): UrlPath<String>,
) -> Response {
    match state.backend.provisioner_test(&provisioner).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct SessionListQuery {
    project_id: Option<String>,
    node_id: Option<String>,
    status: Option<String>,
    limit: Option<u32>,
}

async fn list_sessions<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(params): Query<SessionListQuery>,
) -> Response {
    let statuses = match params.status.as_deref() {
        Some(status) => match SessionStatus::from_str(status) {
            Ok(status) => vec![status],
            Err(err) => return ApiError::bad_request(err.to_string()).into_response(),
        },
        None => Vec::new(),
    };
    let filter = SessionFilter {
        project_id: params.project_id,
        node_id: params.node_id,
        statuses,
        limit: Some(params.limit.unwrap_or(100)),
    };
    match state.backend.db().list_sessions(&filter).await {
        Ok(sessions) => Json(SessionListResponse { sessions }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct EventsQuery {
    after_cursor: Option<u64>,
    limit: Option<u32>,
}

async fn session_events<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(session_id): UrlPath<String>,
    Query(params): Query<EventsQuery>,
) -> Response {
    match state
        .backend
        .db()
        .session_events_after(
            &session_id,
            params.after_cursor.unwrap_or(0),
            params.limit.unwrap_or(REPLAY_BATCH_SIZE),
        )
        .await
    {
        Ok(batch) => Json(batch).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct DiffQuery {
    scope: Option<String>,
    base: Option<String>,
}

async fn session_diff<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(session_id): UrlPath<String>,
    Query(params): Query<DiffQuery>,
) -> Response {
    let scope = match params.scope.as_deref() {
        // 設計上の表記 (`scope=branch`) と enum 名の双方を受け付ける
        None | Some("") | Some("uncommitted") => DiffScope::Uncommitted,
        Some("branch") | Some("branch_base") | Some("branchbase") => DiffScope::BranchBase,
        Some(other) => {
            return ApiError::bad_request(format!("unknown diff scope: {other}")).into_response();
        }
    };
    match state
        .backend
        .workspace_diff(&session_id, scope, params.base)
        .await
    {
        Ok(diff) => Json(diff).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn inbox<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    match state.backend.db().pending_permissions().await {
        Ok(requests) => Json(InboxResponse { requests }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct SearchBody {
    query: Option<String>,
    limit: Option<u32>,
}

async fn search<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(params): Query<SearchQuery>,
    body: Option<Json<SearchBody>>,
) -> Response {
    let body = body.map(|Json(body)| body);
    let query = params
        .q
        .or_else(|| body.as_ref().and_then(|body| body.query.clone()))
        .unwrap_or_default();
    let limit = params
        .limit
        .or_else(|| body.as_ref().and_then(|body| body.limit))
        .unwrap_or(50);

    match state.backend.db().search(&query, limit).await {
        Ok(hits) => Json(SearchResponse { hits }).into_response(),
        Err(fxg_db::DbError::EmptySearchQuery) => {
            ApiError::bad_request("search query must not be empty").into_response()
        }
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct AuditQuery {
    limit: Option<u32>,
}

async fn audit_logs<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(params): Query<AuditQuery>,
) -> Response {
    match state
        .backend
        .db()
        .audit_logs(params.limit.unwrap_or(50))
        .await
    {
        Ok(logs) => Json(AuditLogsResponse { logs }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn create_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<CreateSessionRequest>,
) -> Response {
    match state.backend.create_session(request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn respond_permission<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((session_id, request_id)): UrlPath<(String, String)>,
    Json(request): Json<RespondPermissionRequest>,
) -> Response {
    match state
        .backend
        .respond_permission(&session_id, &request_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn revert_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(session_id): UrlPath<String>,
    Json(request): Json<SessionRevertRequest>,
) -> Response {
    match state
        .backend
        .revert_session(&session_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn resume_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(session_id): UrlPath<String>,
    Json(request): Json<ResumeSessionRequest>,
) -> Response {
    match state
        .backend
        .resume_session(&session_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn list_worktrees<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(project_id): UrlPath<String>,
) -> Response {
    match state.backend.list_worktrees(&project_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn create_worktree<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(project_id): UrlPath<String>,
    Json(request): Json<CreateWorktreeRequest>,
) -> Response {
    match state
        .backend
        .create_worktree(&project_id, request, client)
        .await
    {
        Ok(worktree) => Json(worktree).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn remove_worktree<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(project_id): UrlPath<String>,
    Json(request): Json<RemoveWorktreeRequest>,
) -> Response {
    match state
        .backend
        .remove_worktree(&project_id, request, client)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn prune_worktrees<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(project_id): UrlPath<String>,
    Json(request): Json<PruneWorktreesRequest>,
) -> Response {
    match state
        .backend
        .prune_worktrees(&project_id, request, client)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn scan_projects<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ProjectScanRequest>,
) -> Response {
    match state.backend.scan_projects(&node_id, request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn link_project<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ProjectLinkRequest>,
) -> Response {
    match state.backend.link_project(&node_id, request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn list_agents<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    match state.backend.list_agents().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn install_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
) -> Response {
    match state
        .backend
        .manage_agent(&node_id, AgentAction::Install { agent_id }, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn update_agents<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<UpdateAgentsRequest>,
) -> Response {
    match state
        .backend
        .manage_agent(
            &node_id,
            AgentAction::Update {
                agent_id: request.agent_id,
            },
            client,
        )
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn remove_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
) -> Response {
    match state
        .backend
        .manage_agent(&node_id, AgentAction::Remove { agent_id }, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn rotate_auth_token<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
) -> Response {
    match state.backend.rotate_auth_token(client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn node_tokens<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    match state.backend.node_tokens().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn issue_node_token<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<IssueNodeTokenRequest>,
) -> Response {
    match state.backend.issue_node_token(request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn revoke_node_token<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state.backend.revoke_node_token(&node_id, client).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn kill_switch<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<KillSwitchRequest>,
) -> Response {
    let reason = if request.reason.trim().is_empty() {
        "kill switch".to_owned()
    } else {
        request.reason
    };
    match state.backend.kill_switch(&reason, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn push_subscribe<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<PushSubscribeRequest>,
) -> Response {
    match state.backend.push_subscribe(request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

// ----------------------------------------------------------------------
// WebSocket アップグレード
// ----------------------------------------------------------------------

async fn client_ws<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    ws: WebSocketUpgrade,
) -> Response {
    let backend = state.backend.clone();
    ws.on_upgrade(move |socket| ws::handle_client_ws(backend, socket))
}

async fn pty_ws<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    ws: WebSocketUpgrade,
) -> Response {
    let backend = state.backend.clone();
    ws.on_upgrade(move |socket| pty::handle_pty_ws(backend, socket))
}

// ----------------------------------------------------------------------
// ヘルパー
// ----------------------------------------------------------------------

/// WS 送信の共通ヘルパー (`send_json`)。
pub(crate) async fn send_ws_json<T: serde::Serialize>(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: &T,
) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|err| {
        tracing::warn!("failed to encode ws message: {err}");
    })?;
    sender
        .send(Message::Text(text.into()))
        .await
        .map_err(|err| {
            tracing::debug!("ws send failed: {err}");
        })
}

/// `cursor` 以降のイベントを `SessionEventBatch` として送り切る。
pub(crate) async fn replay_events<B: ClientApiBackend>(
    backend: &B,
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    cursor: &mut u64,
) -> Result<(), ()> {
    loop {
        let batch: SessionEventBatch = backend
            .db()
            .events_after_cursor(*cursor, REPLAY_BATCH_SIZE)
            .await
            .map_err(|err| {
                tracing::warn!("failed to load events for replay: {err}");
            })?;
        if batch.events.is_empty() {
            return Ok(());
        }
        let full_batch = batch.events.len() as u32 >= REPLAY_BATCH_SIZE;
        *cursor = (*cursor).max(batch.cursor);
        send_ws_json(
            sender,
            &ServerWsMessage::EventBatch {
                events: batch.events,
                cursor: batch.cursor,
            },
        )
        .await?;
        if !full_batch {
            return Ok(());
        }
    }
}

/// `Request` が WebSocket アップグレードかどうか。
pub(crate) fn is_websocket_upgrade(request: &Request) -> bool {
    request
        .headers()
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}
