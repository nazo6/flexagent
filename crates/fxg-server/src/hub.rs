//! Node Hub (`/api/v1/node/ws`): 常駐ノードとの Outbound WebSocket 接続管理。
//!
//! 設計: `docs/01-architecture-and-sync.md` §2.2・§3、`docs/03-protocol-and-api.md` §2。
//!
//! - **認証**: `Authorization: Bearer <NODE_TOKEN>` をハッシュ化し、
//!   `server.db.nodes.token_hash` と照合したうえで `NodeHello.node_id` が
//!   トークン発行対象ノードと一致することを検証する (§7.3)
//! - **ハンドシェイク**: `NodeHello` → ハブ側で欠落・遅延しているセッションに
//!   `ResyncRequest` を返し、ノードは指定 `from_node_seq` 以降を再送する
//! - **イベント取り込み**: `EventBatchPush` を `INSERT OR IGNORE` +
//!   投影更新 (同一Tx) で冪等適用し、`EventBatchAck` (水位) を返す。
//!   新規イベントは Client WS 購読者へ配信する
//! - **コマンド中継**: `command_id` で相関を取ってノードの `CommandResult` /
//!   `GitDiffResult` / `WorktreeResult` を要求元クライアントへ返す
//! - **PTY 中継**: `PtyOutput` / `PtyExit` / `PtyError` を `pty_id` 単位で
//!   クライアントの PTY WS へ転送する

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use fxg_db::Db;
use fxg_protocol::client_api::WorktreeInfo;
use fxg_protocol::common::{CommandResult, ErrorCode, WorkspaceDiffResponse};
use fxg_protocol::node_server::{NodeToServerMsg, ResyncTarget, ServerToNodeMsg};
use fxg_protocol::util::uuid_v7;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot};

use crate::api::auth::{hash_token, token_matches};
use crate::api::{ApiError, ClientEvent, PtyChannelError, PtyChannelEvent, send_ws_json};
use crate::state::ServerState;

/// ノード登録後のハンドシェイク (`NodeHello`) 待ちタイムアウト。
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// コマンド中継の応答待ちタイムアウト。
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// プロジェクト操作 (スキャン等) の応答待ちタイムアウト。
///
/// ディレクトリ走査は対象規模により 30 秒を超えることがある。
const PROJECT_OP_TIMEOUT: Duration = Duration::from_secs(120);

/// ACP Registry カタログ取得の応答待ちタイムアウト
/// (インデックスのネットワーク再取得を含む)。
const AGENTS_TIMEOUT: Duration = Duration::from_secs(120);

/// エージェントのインストール・更新の応答待ちタイムアウト。
///
/// バイナリのダウンロード・展開は数分かかりうる。
const AGENT_OP_TIMEOUT: Duration = Duration::from_secs(600);

/// ノード接続への送信キュー容量。
const NODE_CHANNEL_CAPACITY: usize = 512;

/// PTY 出力のブロードキャスト容量。
const PTY_CHANNEL_CAPACITY: usize = 1024;

/// 中継待ちの応答種別。
enum PendingResponse {
    /// コマンド実行結果 (`CommandResult`)
    Command(CommandResult),
    /// Git Diff 取得結果 (`GitDiffResult`)
    GitDiff {
        /// 取得成功時の Diff
        diff: Option<WorkspaceDiffResponse>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// Worktree 操作結果 (`WorktreeResult`)
    Worktree {
        /// 成功時の Worktree 情報 (削除時は `None`)
        worktree: Option<WorktreeInfo>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// セッション Revert 結果 (`RevertResult`)
    Revert {
        /// 成功したか
        success: bool,
        /// 失敗時の構造化エラーコード
        code: Option<ErrorCode>,
        /// 成功時の復元結果
        outcome: Option<fxg_protocol::client_api::SessionRevertResponse>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// セッション再開結果 (`ResumeResult`)
    Resume {
        /// 成功したか
        success: bool,
        /// 失敗時の構造化エラーコード
        code: Option<ErrorCode>,
        /// 成功時のネイティブ復元成否 (`false` = 履歴 Replay で継続)
        context_restored: Option<bool>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// ファイルシステム閲覧結果 (`BrowseFsResult`)
    BrowseFs {
        /// 取得成功時のブラウズ結果
        response: Option<fxg_protocol::client_api::FsBrowseResponse>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// プロジェクト一括スキャン結果 (`ProjectScanResult`)
    ProjectScan {
        /// 成功時のスキャン結果
        response: Option<fxg_protocol::client_api::ProjectScanResponse>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// プロジェクト手動紐付け結果 (`ProjectLinkResult`)
    ProjectLink {
        /// 成功時の紐付け結果
        response: Option<fxg_protocol::client_api::ProjectLinkResponse>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// エージェント管理操作結果 (`AgentOpResult`)
    AgentOp {
        /// 成功したか
        success: bool,
        /// 失敗時の構造化エラーコード
        code: Option<ErrorCode>,
        /// 成功時の表示メッセージ
        message: Option<String>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// ACP Registry カタログ結果 (`AgentsResult`)
    Agents {
        /// 取得成功時のカタログ
        response: Option<fxg_protocol::client_api::AgentsResponse>,
        /// 失敗時のメッセージ
        error: Option<String>,
    },
    /// ノード切断などによる失敗
    Failed {
        /// 構造化エラーコード
        code: ErrorCode,
        /// 人間向けメッセージ
        message: String,
    },
}

/// 中継待ちエントリ。
struct PendingEntry {
    /// 応答を待っているノード
    node_id: String,
    /// 応答の送り先
    tx: oneshot::Sender<PendingResponse>,
}

/// 接続中のノード 1 件。
#[derive(Clone)]
struct NodeConnection {
    /// 接続世代 ID (再接続時に古い接続の後処理を無効化する)
    conn_id: String,
    /// 送信キュー
    tx: mpsc::Sender<ServerToNodeMsg>,
}

/// PTY の中継先。
struct PtyRoute {
    /// 所有ノードID
    node_id: String,
    /// クライアントへ配信するブロードキャスト
    tx: broadcast::Sender<PtyChannelEvent>,
}

struct HubInner {
    db: Db,
    client_events: broadcast::Sender<ClientEvent>,
    nodes: RwLock<HashMap<String, NodeConnection>>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    ptys: Mutex<HashMap<String, PtyRoute>>,
}

/// ノード接続レジストリとコマンド中継 (中央サーバーの共有状態から参照される)。
#[derive(Clone)]
pub struct NodeHub {
    inner: Arc<HubInner>,
}

impl NodeHub {
    /// ハブを作成する。
    pub(crate) fn new(db: Db, client_events: broadcast::Sender<ClientEvent>) -> Self {
        Self {
            inner: Arc::new(HubInner {
                db,
                client_events,
                nodes: RwLock::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                ptys: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// サーバーDB。
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// Client WS 配信用ブロードキャスト。
    pub(crate) fn client_events(&self) -> &broadcast::Sender<ClientEvent> {
        &self.inner.client_events
    }

    /// ノードが接続中か。
    pub async fn is_online(&self, node_id: &str) -> bool {
        self.inner.nodes.read().await.contains_key(node_id)
    }

    /// 接続中ノードの一覧。
    pub async fn online_nodes(&self) -> Vec<String> {
        self.inner.nodes.read().await.keys().cloned().collect()
    }

    /// 一時VM (`fxg daemon --stdio`) の接続を登録する。
    ///
    /// 返り値の受信キューへ流れた `ServerToNodeMsg` を、プロビジョナーマネージャが
    /// 子プロセスの `stdin` (JSON Lines) へ転送する。
    pub(crate) async fn attach_ephemeral(
        &self,
        node_id: &str,
    ) -> (mpsc::Receiver<ServerToNodeMsg>, String) {
        let (tx, rx) = mpsc::channel(NODE_CHANNEL_CAPACITY);
        let conn_id = uuid_v7();
        self.register(node_id, &conn_id, tx).await;
        tracing::info!(node_id, "ephemeral node attached (stdio)");
        (rx, conn_id)
    }

    /// 一時VMの接続を解除する (子プロセス終了時)。
    pub(crate) async fn detach_ephemeral(&self, node_id: &str, exit_code: Option<i32>) {
        // 接続世代が不明なため、登録されている接続を直接取り除いて後処理する
        let conn_id = {
            let nodes = self.inner.nodes.read().await;
            nodes.get(node_id).map(|entry| entry.conn_id.clone())
        };
        if let Some(conn_id) = conn_id {
            self.disconnect(node_id, &conn_id).await;
        }
        if let Err(err) = self.db().set_node_online(node_id, false).await {
            tracing::debug!(node_id, "failed to mark ephemeral node offline: {err}");
        }
        if let Err(err) = self
            .db()
            .set_node_lifecycle(
                node_id,
                fxg_protocol::common::NodeLifecycleStatus::Terminated,
            )
            .await
        {
            tracing::debug!(node_id, "failed to mark ephemeral node terminated: {err}");
        }
        tracing::info!(node_id, exit_code, "ephemeral node detached (stdio)");
    }

    /// 一時VMの接続を無条件で取り除く (サーバーシャットダウン等)。
    pub(crate) async fn detach_all_ephemeral(&self, node_ids: &[String]) {
        for node_id in node_ids {
            self.detach_ephemeral(node_id, None).await;
        }
    }

    /// ノード接続を登録する (既存接続は置き換え)。
    async fn register(&self, node_id: &str, conn_id: &str, tx: mpsc::Sender<ServerToNodeMsg>) {
        let mut nodes = self.inner.nodes.write().await;
        if nodes
            .insert(
                node_id.to_owned(),
                NodeConnection {
                    conn_id: conn_id.to_owned(),
                    tx,
                },
            )
            .is_some()
        {
            tracing::info!(node_id, "node reconnected; replaced previous connection");
        }
    }

    /// ノード接続を解除する (同一接続世代のみ)。未完了の中継と PTY を失敗させる。
    async fn disconnect(&self, node_id: &str, conn_id: &str) {
        {
            let mut nodes = self.inner.nodes.write().await;
            let is_current = nodes
                .get(node_id)
                .map(|entry| entry.conn_id == conn_id)
                .unwrap_or(false);
            if is_current {
                nodes.remove(node_id);
            }
        }

        // 未完了の中継へ NODE_OFFLINE を返す
        let mut pending = self.inner.pending.lock().await;
        let failed: Vec<String> = pending
            .iter()
            .filter(|(_, entry)| entry.node_id == node_id)
            .map(|(command_id, _)| command_id.clone())
            .collect();
        for command_id in failed {
            if let Some(entry) = pending.remove(&command_id) {
                let _ = entry.tx.send(PendingResponse::Failed {
                    code: ErrorCode::NodeOffline,
                    message: format!("node {node_id} disconnected"),
                });
            }
        }
        drop(pending);

        // そのノードが所有する PTY を終了扱いにする
        let mut ptys = self.inner.ptys.lock().await;
        let pty_ids: Vec<String> = ptys
            .iter()
            .filter(|(_, route)| route.node_id == node_id)
            .map(|(pty_id, _)| pty_id.clone())
            .collect();
        for pty_id in pty_ids {
            if let Some(route) = ptys.remove(&pty_id) {
                let _ = route.tx.send(PtyChannelEvent::Exit { exit_code: None });
            }
        }
    }

    /// ノードへメッセージを送る。未接続時は `NODE_OFFLINE`。
    pub async fn send(&self, node_id: &str, message: ServerToNodeMsg) -> Result<(), ApiError> {
        let tx = {
            let nodes = self.inner.nodes.read().await;
            nodes.get(node_id).map(|entry| entry.tx.clone())
        };
        let Some(tx) = tx else {
            return Err(offline_error(node_id));
        };
        tx.send(message).await.map_err(|_| offline_error(node_id))
    }

    /// コマンドを中継し、`command_id` 相関で `CommandResult` を待つ。
    pub async fn command(
        &self,
        node_id: &str,
        command_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<CommandResult, ApiError> {
        let response = self
            .request(node_id, command_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::Command(result) => Ok(result),
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Git Diff を中継し、`request_id` 相関で結果を待つ。
    pub async fn git_diff(
        &self,
        node_id: &str,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<WorkspaceDiffResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::GitDiff { diff, error } => match (diff, error) {
                (Some(diff), _) => Ok(diff),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no diff")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// Worktree 操作を中継し、`command_id` 相関で結果を待つ。
    pub async fn worktree_op(
        &self,
        node_id: &str,
        command_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<Option<WorktreeInfo>, ApiError> {
        let response = self
            .request(node_id, command_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::Worktree { worktree, error } => match (worktree, error) {
                (Some(worktree), _) => Ok(Some(worktree)),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Ok(None),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// セッション Revert を中継し、`command_id` 相関で結果を待つ。
    pub async fn revert_session(
        &self,
        node_id: &str,
        command_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::SessionRevertResponse, ApiError> {
        let response = self
            .request(node_id, command_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::Revert {
                success,
                code,
                outcome,
                error,
            } => {
                if success {
                    outcome.ok_or_else(|| ApiError::internal("node returned no revert outcome"))
                } else {
                    Err(ApiError::from_code(
                        code.unwrap_or(ErrorCode::Internal),
                        error.unwrap_or_else(|| "revert failed".to_owned()),
                    ))
                }
            }
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// セッション再開を中継し、`command_id` 相関で結果を待つ。
    ///
    /// 戻り値はネイティブ復元の成否 (`false` = 履歴 Replay で継続)。
    pub async fn resume_session(
        &self,
        node_id: &str,
        command_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<bool, ApiError> {
        let response = self
            .request(node_id, command_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::Resume {
                success,
                code,
                context_restored,
                error,
            } => {
                if success {
                    context_restored
                        .ok_or_else(|| ApiError::internal("node returned no resume outcome"))
                } else {
                    Err(ApiError::from_code(
                        code.unwrap_or(ErrorCode::Internal),
                        error.unwrap_or_else(|| "resume failed".to_owned()),
                    ))
                }
            }
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// ファイルシステム閲覧を中継し、`request_id` 相関で結果を待つ。
    pub async fn browse_fs(
        &self,
        node_id: &str,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::FsBrowseResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::BrowseFs { response, error } => match (response, error) {
                (Some(resp), _) => Ok(resp),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no fs browse response")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// プロジェクト一括スキャンを中継し、`request_id` 相関で結果を待つ。
    ///
    /// ディレクトリ走査は時間がかかりうるため、通常コマンドより長い
    /// [`PROJECT_OP_TIMEOUT`] で待つ。
    pub async fn scan_projects(
        &self,
        node_id: &str,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::ProjectScanResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, PROJECT_OP_TIMEOUT)
            .await?;
        match response {
            PendingResponse::ProjectScan { response, error } => match (response, error) {
                (Some(resp), _) => Ok(resp),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no project scan response")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// プロジェクト手動紐付けを中継し、`request_id` 相関で結果を待つ。
    pub async fn link_project(
        &self,
        node_id: &str,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::ProjectLinkResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, COMMAND_TIMEOUT)
            .await?;
        match response {
            PendingResponse::ProjectLink { response, error } => match (response, error) {
                (Some(resp), _) => Ok(resp),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no project link response")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// ACP Registry カタログを中継し、`request_id` 相関で結果を待つ。
    pub async fn agents(
        &self,
        node_id: &str,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<fxg_protocol::client_api::AgentsResponse, ApiError> {
        let response = self
            .request(node_id, request_id, message, AGENTS_TIMEOUT)
            .await?;
        match response {
            PendingResponse::Agents { response, error } => match (response, error) {
                (Some(resp), _) => Ok(resp),
                (None, Some(error)) => Err(ApiError::bad_request(error)),
                (None, None) => Err(ApiError::internal("node returned no agents response")),
            },
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// エージェント管理操作 (install / update / remove) を中継し、
    /// `request_id` 相関で結果を待つ。成功時は表示メッセージを返す。
    pub async fn manage_agent(
        &self,
        node_id: &str,
        request_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<Option<String>, ApiError> {
        let response = self
            .request(node_id, request_id, message, AGENT_OP_TIMEOUT)
            .await?;
        match response {
            PendingResponse::AgentOp {
                success,
                code,
                message,
                error,
            } => {
                if success {
                    Ok(message)
                } else {
                    Err(ApiError::from_code(
                        code.unwrap_or(ErrorCode::Internal),
                        error.unwrap_or_else(|| "agent operation failed".to_owned()),
                    ))
                }
            }
            PendingResponse::Failed { code, message } => Err(ApiError::from_code(code, message)),
            _ => Err(ApiError::internal("unexpected node response kind")),
        }
    }

    /// 送信 → 相関待ちの共通処理。
    async fn request(
        &self,
        node_id: &str,
        correlation_id: &str,
        message: ServerToNodeMsg,
        timeout: Duration,
    ) -> Result<PendingResponse, ApiError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.inner.pending.lock().await;
            if pending.contains_key(correlation_id) {
                return Err(ApiError::from_code(
                    ErrorCode::CommandDuplicate,
                    format!("duplicate in-flight command id: {correlation_id}"),
                ));
            }
            pending.insert(
                correlation_id.to_owned(),
                PendingEntry {
                    node_id: node_id.to_owned(),
                    tx,
                },
            );
        }

        if let Err(err) = self.send(node_id, message).await {
            self.inner.pending.lock().await.remove(correlation_id);
            return Err(err);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_canceled)) => Err(ApiError::from_code(
                ErrorCode::NodeOffline,
                format!("node {node_id} disconnected"),
            )),
            Err(_) => {
                self.inner.pending.lock().await.remove(correlation_id);
                Err(ApiError::internal(format!(
                    "timed out waiting for node response: {correlation_id}"
                )))
            }
        }
    }

    /// 中継待ちの応答を解決する。
    async fn resolve(&self, correlation_id: &str, response: PendingResponse) {
        let entry = self.inner.pending.lock().await.remove(correlation_id);
        match entry {
            Some(entry) => {
                let _ = entry.tx.send(response);
            }
            None => {
                tracing::debug!(correlation_id, "late node response (no waiter)");
            }
        }
    }

    /// 全接続ノードへメッセージを配信し、送信先のノードID一覧を返す。
    pub async fn broadcast(&self, message: ServerToNodeMsg) -> Vec<String> {
        let connections: Vec<(String, mpsc::Sender<ServerToNodeMsg>)> = {
            let nodes = self.inner.nodes.read().await;
            nodes
                .iter()
                .map(|(node_id, entry)| (node_id.clone(), entry.tx.clone()))
                .collect()
        };
        let mut delivered = Vec::new();
        for (node_id, tx) in connections {
            if tx.send(message.clone()).await.is_ok() {
                delivered.push(node_id);
            }
        }
        delivered
    }

    // ------------------------------------------------------------------
    // PTY 中継
    // ------------------------------------------------------------------

    /// PTY の中継先を登録する (spawn 時)。
    pub(crate) async fn insert_pty(&self, pty_id: &str, node_id: &str) {
        let (tx, _) = broadcast::channel(PTY_CHANNEL_CAPACITY);
        self.inner.ptys.lock().await.insert(
            pty_id.to_owned(),
            PtyRoute {
                node_id: node_id.to_owned(),
                tx,
            },
        );
    }

    /// PTY 出力を購読する (未登録の場合は `None`)。
    pub(crate) async fn pty_subscribe(
        &self,
        pty_id: &str,
    ) -> Option<broadcast::Receiver<PtyChannelEvent>> {
        self.inner
            .ptys
            .lock()
            .await
            .get(pty_id)
            .map(|route| route.tx.subscribe())
    }

    /// PTY を所有するノードへメッセージを中継する。
    pub(crate) async fn pty_send(
        &self,
        pty_id: &str,
        message: ServerToNodeMsg,
    ) -> Result<(), PtyChannelError> {
        let node_id = {
            let ptys = self.inner.ptys.lock().await;
            ptys.get(pty_id).map(|route| route.node_id.clone())
        };
        let Some(node_id) = node_id else {
            return Err(PtyChannelError::not_found(format!(
                "pty session not found: {pty_id}"
            )));
        };
        self.send(&node_id, message)
            .await
            .map_err(|err| PtyChannelError::new(err.code.as_str(), err.message))
    }

    /// PTY の中継登録を解除する (kill / Exit 後)。
    pub(crate) async fn remove_pty(&self, pty_id: &str) {
        self.inner.ptys.lock().await.remove(pty_id);
    }

    /// ノードから届いた PTY イベントを購読者へ配信する。
    async fn publish_pty(&self, pty_id: &str, event: PtyChannelEvent) {
        let route = {
            let ptys = self.inner.ptys.lock().await;
            ptys.get(pty_id).map(|route| route.tx.clone())
        };
        if let Some(tx) = route {
            let _ = tx.send(event);
        }
    }
}

/// `NODE_OFFLINE` エラーを生成する。
fn offline_error(node_id: &str) -> ApiError {
    ApiError::from_code(
        ErrorCode::NodeOffline,
        format!("node is not connected: {node_id}"),
    )
}

// ----------------------------------------------------------------------
// WebSocket ハンドラ
// ----------------------------------------------------------------------

/// `/api/v1/node/ws` ハンドラ (ノード個別トークン認証 + WS アップグレード)。
pub(crate) async fn node_ws(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(provided) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
    else {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "missing node token",
        )
        .into_response();
    };

    // 平文トークンは保存していないため、ハッシュ化して定数時間比較で照合する
    let provided_hash = hash_token(provided);
    let tokens = match state.db().list_node_tokens().await {
        Ok(tokens) => tokens,
        Err(err) => return ApiError::internal(err).into_response(),
    };
    let owner = tokens
        .into_iter()
        .find(|(_, stored)| token_matches(stored, &provided_hash))
        .map(|(node_id, _)| node_id);
    let Some(owner) = owner else {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "invalid node token",
        )
        .into_response();
    };

    ws.on_upgrade(move |socket| handle_node_connection(state, owner, socket))
}

/// ノード接続のメインループ (NodeHello → 登録 → 双方向メッセージ)。
async fn handle_node_connection(state: ServerState, token_owner: String, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();

    // 1. 最初のメッセージは NodeHello (タイムアウト付き)
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, receiver.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<NodeToServerMsg>(&text) {
            Ok(hello @ NodeToServerMsg::NodeHello { .. }) => hello,
            Ok(other) => {
                tracing::warn!(op = ?std::mem::discriminant(&other), "node sent a non-hello first message");
                let _ = sender.close().await;
                return;
            }
            Err(err) => {
                tracing::warn!("invalid node hello: {err}");
                let _ = sender.close().await;
                return;
            }
        },
        _ => {
            tracing::warn!("node hello timeout or connection closed");
            return;
        }
    };

    let NodeToServerMsg::NodeHello {
        node_id,
        name,
        os,
        arch,
        version,
        is_ephemeral,
        installed_agents,
        projects,
        sessions,
    } = hello
    else {
        unreachable!("hello was matched above");
    };

    // なりすまし拒否: NodeHello.node_id はトークン発行対象と一致すること
    if node_id != token_owner {
        tracing::warn!(
            node_id,
            token_owner,
            "node hello node_id does not match token owner; rejecting"
        );
        let _ = sender.close().await;
        return;
    }

    // 2. ノード登録 (ノードは初回 Hello 以前に自己登録している場合もある)
    let record = fxg_db::NodeRecord {
        node_id: node_id.clone(),
        name,
        os,
        arch,
        version,
        installed_agents,
        is_ephemeral,
        provisioner: None,
        lifecycle_status: fxg_protocol::common::NodeLifecycleStatus::Ready,
        idle_timeout_secs: None,
        is_online: true,
    };
    if let Err(err) = state.db().upsert_node(&record).await {
        tracing::warn!(node_id, "failed to upsert node from hello: {err}");
        let _ = sender.close().await;
        return;
    }
    if let Err(err) = apply_project_reports(&state, &node_id, &projects).await {
        tracing::warn!(node_id, "failed to apply project reports: {err}");
    }

    // 3. 接続登録
    let (tx, mut rx) = mpsc::channel(NODE_CHANNEL_CAPACITY);
    let conn_id = uuid_v7();
    state.hub().register(&node_id, &conn_id, tx).await;
    tracing::info!(node_id, "node connected");

    // 4. 欠落・遅延セッションの ResyncRequest
    let targets = compute_resync(&state, &node_id, &sessions).await;
    if !targets.is_empty() {
        tracing::info!(
            node_id,
            sessions = targets.len(),
            "requesting session resync from node"
        );
        let _ = state
            .hub()
            .send(
                &node_id,
                ServerToNodeMsg::ResyncRequest { sessions: targets },
            )
            .await;
    }

    // 5. 双方向メッセージループ
    loop {
        tokio::select! {
            outgoing = rx.recv() => match outgoing {
                Some(message) => {
                    if send_ws_json(&mut sender, &message).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    handle_node_message(&state, &node_id, &text).await;
                }
                Some(Ok(Message::Ping(payload))) => {
                    if sender.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }

    // 6. 切断処理 (同一接続世代のみ有効)
    state.hub().disconnect(&node_id, &conn_id).await;
    if let Err(err) = state.db().set_node_online(&node_id, false).await {
        tracing::warn!(node_id, "failed to mark node offline: {err}");
    }
    tracing::info!(node_id, "node disconnected");
}

/// ノードからのメッセージを処理する。
async fn handle_node_message(state: &ServerState, node_id: &str, text: &str) {
    let message: NodeToServerMsg = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(err) => {
            tracing::warn!(node_id, "invalid node message: {err}");
            return;
        }
    };

    match message {
        NodeToServerMsg::NodeHello {
            node_id: hello_node_id,
            name,
            os,
            arch,
            version,
            is_ephemeral,
            installed_agents,
            projects,
            sessions,
        } => {
            // 再接続ではなく実行中の再送 (Worktree 変更時など)
            if hello_node_id != node_id {
                tracing::warn!(
                    node_id,
                    hello_node_id,
                    "node hello node_id mismatch; ignoring"
                );
                return;
            }
            let record = fxg_db::NodeRecord {
                node_id: node_id.to_owned(),
                name,
                os,
                arch,
                version,
                installed_agents,
                is_ephemeral,
                provisioner: None,
                lifecycle_status: fxg_protocol::common::NodeLifecycleStatus::Ready,
                idle_timeout_secs: None,
                is_online: true,
            };
            if let Err(err) = state.db().upsert_node(&record).await {
                tracing::warn!(node_id, "failed to update node from hello: {err}");
            }
            if let Err(err) = apply_project_reports(state, node_id, &projects).await {
                tracing::warn!(node_id, "failed to apply project reports: {err}");
            }
            let targets = compute_resync(state, node_id, &sessions).await;
            if !targets.is_empty() {
                let _ = state
                    .hub()
                    .send(
                        node_id,
                        ServerToNodeMsg::ResyncRequest { sessions: targets },
                    )
                    .await;
            }
        }
        NodeToServerMsg::EventBatchPush { events } => {
            apply_event_batch(state, node_id, events).await;
        }
        NodeToServerMsg::LiveStreamDelta { session_id, delta } => {
            let _ = state
                .hub()
                .client_events()
                .send(ClientEvent::StreamDelta { session_id, delta });
        }
        NodeToServerMsg::CommandResult {
            command_id,
            success,
            code,
            error,
            session_id,
        } => {
            let result = CommandResult {
                command_id: command_id.clone(),
                success,
                code,
                error,
                session_id,
            };
            state
                .hub()
                .resolve(&command_id, PendingResponse::Command(result))
                .await;
        }
        NodeToServerMsg::GitDiffResult {
            request_id,
            diff,
            error,
        } => {
            state
                .hub()
                .resolve(&request_id, PendingResponse::GitDiff { diff, error })
                .await;
        }
        NodeToServerMsg::WorktreeResult {
            command_id,
            worktree,
            error,
        } => {
            state
                .hub()
                .resolve(&command_id, PendingResponse::Worktree { worktree, error })
                .await;
        }
        NodeToServerMsg::RevertResult {
            command_id,
            success,
            code,
            outcome,
            error,
        } => {
            state
                .hub()
                .resolve(
                    &command_id,
                    PendingResponse::Revert {
                        success,
                        code,
                        outcome,
                        error,
                    },
                )
                .await;
        }
        NodeToServerMsg::ResumeResult {
            command_id,
            success,
            code,
            context_restored,
            error,
        } => {
            state
                .hub()
                .resolve(
                    &command_id,
                    PendingResponse::Resume {
                        success,
                        code,
                        context_restored,
                        error,
                    },
                )
                .await;
        }
        NodeToServerMsg::BrowseFsResult {
            request_id,
            response,
            error,
        } => {
            state
                .hub()
                .resolve(&request_id, PendingResponse::BrowseFs { response, error })
                .await;
        }
        NodeToServerMsg::ProjectScanResult {
            request_id,
            response,
            error,
        } => {
            state
                .hub()
                .resolve(
                    &request_id,
                    PendingResponse::ProjectScan { response, error },
                )
                .await;
        }
        NodeToServerMsg::ProjectLinkResult {
            request_id,
            response,
            error,
        } => {
            state
                .hub()
                .resolve(
                    &request_id,
                    PendingResponse::ProjectLink { response, error },
                )
                .await;
        }
        NodeToServerMsg::AgentsResult {
            request_id,
            response,
            error,
        } => {
            state
                .hub()
                .resolve(&request_id, PendingResponse::Agents { response, error })
                .await;
        }
        NodeToServerMsg::AgentOpResult {
            request_id,
            success,
            code,
            message,
            error,
        } => {
            state
                .hub()
                .resolve(
                    &request_id,
                    PendingResponse::AgentOp {
                        success,
                        code,
                        message,
                        error,
                    },
                )
                .await;
        }
        NodeToServerMsg::PtyOutput { pty_id, data_b64 } => {
            use base64::Engine as _;
            match base64::engine::general_purpose::STANDARD.decode(data_b64.as_bytes()) {
                Ok(data) => {
                    state
                        .hub()
                        .publish_pty(&pty_id, PtyChannelEvent::Output { data })
                        .await;
                }
                Err(err) => {
                    tracing::warn!(pty_id, "invalid pty output base64: {err}");
                }
            }
        }
        NodeToServerMsg::PtyExit { pty_id, exit_code } => {
            state
                .hub()
                .publish_pty(&pty_id, PtyChannelEvent::Exit { exit_code })
                .await;
            state.hub().remove_pty(&pty_id).await;
        }
        NodeToServerMsg::PtyError {
            pty_id,
            code,
            message,
        } => {
            state
                .hub()
                .publish_pty(&pty_id, PtyChannelEvent::Error { code, message })
                .await;
            state.hub().remove_pty(&pty_id).await;
        }
        NodeToServerMsg::GitCredentialRequest {
            request_id,
            repo_url,
            operation,
        } => {
            // Git Credential Proxy (一時VMの GIT_ASKPASS): 設定されたプロバイダから
            // オンメモリでトークンを解決し、stdio パイプ上で返す (docs/01 §6.4)
            let response = crate::provisioner::resolve_git_token(state, &repo_url).await;
            let message = match response {
                Some(response) => ServerToNodeMsg::GitCredentialResponse {
                    request_id,
                    username: response.username,
                    token: response.token,
                    error: response.error,
                },
                None => ServerToNodeMsg::GitCredentialResponse {
                    request_id,
                    username: fxg_protocol::config::DEFAULT_GIT_USERNAME.to_owned(),
                    token: None,
                    error: Some(format!(
                        "no git credential configured for {repo_url} ({operation})"
                    )),
                },
            };
            let _ = state.hub().send(node_id, message).await;
        }
        NodeToServerMsg::WorkspaceBundleUpload {
            session_id,
            branch,
            head_commit,
            bundle_b64,
        } => {
            // 分割転送された bundle を DrainComplete まで蓄積する
            crate::provisioner::on_bundle_upload(
                state,
                &session_id,
                &branch,
                &head_commit,
                &bundle_b64,
            );
        }
        NodeToServerMsg::DrainComplete { node_id: drained } => {
            tracing::info!(node_id, "drain complete");
            crate::provisioner::on_drain_complete(state, &drained).await;
        }
    }
}

/// 一時VM (`fxg daemon --stdio`) から受信したメッセージを処理する。
///
/// WebSocket ノードと同一のメッセージ型・同一の処理経路
/// ([`handle_node_message`]) を使う (設計: docs/03 §2)。
pub(crate) async fn handle_stdio_message(state: &ServerState, node_id: &str, text: &str) {
    handle_node_message(state, node_id, text).await;
}

/// `NodeHello` のプロジェクト報告を `projects` / `project_node_bindings` へ反映する。
async fn apply_project_reports(
    state: &ServerState,
    node_id: &str,
    projects: &[fxg_protocol::common::NodeProjectReport],
) -> Result<(), fxg_db::DbError> {
    for project in projects {
        state
            .db()
            .upsert_project(&fxg_db::registration::ProjectRecord {
                project_id: project.project_id.clone(),
                name: project.name.clone(),
                canonical_git_url: project.canonical_git_url.clone(),
            })
            .await?;
        for binding in &project.bindings {
            state
                .db()
                .upsert_project_binding(&fxg_db::registration::ProjectBindingRecord {
                    project_id: project.project_id.clone(),
                    node_id: node_id.to_owned(),
                    local_path: binding.local_path.clone(),
                    is_worktree: binding.is_worktree,
                    git_branch: binding.git_branch.clone(),
                })
                .await?;
        }
    }
    Ok(())
}

/// ハブ側で欠落・遅延しているセッションの再送指定を計算する。
async fn compute_resync(
    state: &ServerState,
    node_id: &str,
    sessions: &[fxg_protocol::node_server::SessionSyncState],
) -> Vec<ResyncTarget> {
    let mut targets = Vec::new();
    for sync in sessions {
        if sync.last_node_seq == 0 {
            continue;
        }
        match state.db().get_session(&sync.session_id).await {
            Ok(Some(session)) if session.node_id == node_id => {
                if session.last_node_seq < sync.last_node_seq {
                    targets.push(ResyncTarget {
                        session_id: sync.session_id.clone(),
                        from_node_seq: session.last_node_seq + 1,
                    });
                } else if session.last_node_seq > sync.last_node_seq {
                    tracing::warn!(
                        session_id = sync.session_id,
                        hub_seq = session.last_node_seq,
                        node_seq = sync.last_node_seq,
                        "hub is ahead of node for session; skipping resync"
                    );
                }
            }
            Ok(Some(session)) => {
                tracing::warn!(
                    session_id = sync.session_id,
                    hub_node = session.node_id,
                    node_id,
                    "session is owned by a different node; requesting full resync"
                );
                targets.push(ResyncTarget {
                    session_id: sync.session_id.clone(),
                    from_node_seq: 1,
                });
            }
            Ok(None) => {
                // ハブにとって未知のセッション: SessionCreated を含む全量再送
                targets.push(ResyncTarget {
                    session_id: sync.session_id.clone(),
                    from_node_seq: 1,
                });
            }
            Err(err) => {
                tracing::warn!(
                    session_id = sync.session_id,
                    "failed to read session: {err}"
                );
            }
        }
    }
    targets
}

/// `EventBatchPush` を冪等適用し、ACK と Client WS 配信を行う。
///
/// 適用に失敗した場合は ACK を返さない (ノードの水位が進まず再送される)。
async fn apply_event_batch(
    state: &ServerState,
    node_id: &str,
    events: Vec<fxg_protocol::events::SessionEventEnvelope>,
) {
    // なりすまし防止: SessionCreated.node_id は接続元ノードと一致すること
    for event in &events {
        if let fxg_protocol::events::UnifiedEventPayload::SessionCreated {
            node_id: event_node_id,
            ..
        } = &event.payload
            && event_node_id != node_id
        {
            tracing::warn!(
                node_id,
                event_node_id,
                session_id = event.session_id,
                "rejecting event batch with mismatched node_id"
            );
            return;
        }
    }

    // セッション単位に分割 (プロトコル上バッチはセッション単位だが防御的に)
    let mut batches: Vec<(String, Vec<fxg_protocol::events::SessionEventEnvelope>)> = Vec::new();
    for event in events {
        match batches.last_mut() {
            Some((session_id, batch)) if session_id == &event.session_id => batch.push(event),
            _ => batches.push((event.session_id.clone(), vec![event])),
        }
    }

    // Web Push 通知の対象 (新規に適用された承認リクエスト)
    let mut notify_events: Vec<fxg_protocol::events::SessionEventEnvelope> = Vec::new();

    for (session_id, batch) in batches {
        let acked_up_to_node_seq = batch.last().map(|event| event.node_seq).unwrap_or(0);
        match state.db().append_events(&batch).await {
            Ok(outcomes) => {
                for (event, outcome) in batch.iter().zip(outcomes.iter()) {
                    if outcome.inserted {
                        let _ = state.hub().client_events().send(ClientEvent::Persisted {
                            event: Box::new(event.clone()),
                            cursor: outcome.cursor,
                        });
                        if matches!(
                            event.payload,
                            fxg_protocol::events::UnifiedEventPayload::PermissionRequest { .. }
                        ) {
                            notify_events.push(event.clone());
                        }
                    }
                }
                let _ = state
                    .hub()
                    .send(
                        node_id,
                        ServerToNodeMsg::EventBatchAck {
                            session_id,
                            acked_up_to_node_seq,
                        },
                    )
                    .await;
            }
            Err(err) => {
                tracing::warn!(
                    node_id,
                    session_id,
                    "failed to apply event batch (will retry): {err}"
                );
            }
        }
    }

    // 承認リクエストの Web Push 通知 (ACK をブロックしないよう非同期で送る)
    if !notify_events.is_empty() {
        let push = state.push().clone();
        let db = state.db().clone();
        tokio::spawn(async move {
            crate::push::notify_permission_requests(&push, &db, &notify_events).await;
        });
    }
}
