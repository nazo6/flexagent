//! Client (PWA / Browser / CLI) ⇔ Server (または Local Node) API 型定義。
//!
//! 中央サーバー (`fxg server`, `:8080`) とノードのローカルWebサーバー
//! (`fxg daemon`, `127.0.0.1:7860`) は完全に同一の API パスと
//! WebSocket フォーマットを実装する (設計: `docs/03-protocol-and-api.md` §3)。
//! UI 側は接続先 URL を意識せずに同じ型・同じコードで双方へ接続する。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{
    AuditLogEntry, CommandResult, ElicitationAction, ElicitationRequestEntry, ErrorCode,
    NodeSummary, PermissionRequestEntry, ProjectSummary, SearchHit, SessionControlAction,
    SessionSummary, StreamDeltaPayload,
};
use crate::events::SessionEventBatch;

/// 接続先の種別 (`GET /api/v1/system/info`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConnectionRole {
    /// 中央サーバー (`fxg server`) に接続中
    CentralServer,
    /// ノードのローカルWebサーバー (`fxg daemon`) に接続中
    LocalNode,
}

/// `GET /api/v1/system/info` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SystemInfoResponse {
    /// 接続先の種別
    pub role: ConnectionRole,
    /// fxg バイナリバージョン
    pub version: String,
    /// Web Push 用 VAPID 公開鍵 (中央サーバーのみ)
    pub vapid_public_key: Option<String>,
    /// 未同期 Outbox イベント件数 (ローカルノード接続時のみ)
    pub unsynced_event_count: Option<u64>,
    /// 中央サーバーへの WebSocket 接続状態 (ローカルノード接続時のみ)
    pub central_connected: Option<bool>,
}

/// `GET /api/v1/meta` レスポンス (認証不要の接続先メタ情報)。
///
/// `GET /api/v1/system/info` は認証必須のため、未認証の Web UI はこの
/// エンドポイントで接続先種別を判定し、トークン入力ダイアログの表示
/// (中央サーバー / ローカルノード) を正しく切り替える (設計: `docs/03` §3.1)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MetaResponse {
    /// 接続先の種別
    pub role: ConnectionRole,
}

/// REST / WS 共通のエラーレスポンスボディ。
///
/// 形式: `{ "error": { "code": "<ErrorCode>", "message": "..." } }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiErrorResponse {
    /// エラー本体
    pub error: ApiErrorBody,
}

/// エラー本体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiErrorBody {
    /// 構造化エラーコード
    pub code: ErrorCode,
    /// 人間向けメッセージ
    pub message: String,
}

/// `GET /api/v1/projects` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectsResponse {
    /// 論理プロジェクト一覧 (ノード・Worktree 紐付け含む)
    pub projects: Vec<ProjectSummary>,
}

/// `GET /api/v1/nodes` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodesResponse {
    /// ノード一覧 (オンライン状態・一時ノード属性含む)
    pub nodes: Vec<NodeSummary>,
}

/// 利用可能な一時VM・サンドボックスプロビジョナー (`GET /api/v1/provisioners`)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProvisionerSummary {
    /// プロビジョナー識別子 (`local-docker` / `colab-pro` 等)
    pub name: String,
    /// 説明
    pub description: Option<String>,
    /// アイドル自動破棄までの秒数
    pub idle_timeout_secs: Option<u64>,
}

/// `GET /api/v1/provisioners` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProvisionersResponse {
    /// プロビジョナー一覧
    pub provisioners: Vec<ProvisionerSummary>,
}

/// `POST /api/v1/provisioners/:name/test` レスポンス (疎通検証)。
///
/// プロビジョナーを起動し、`fxg daemon --stdio` の `NodeHello`
/// ハンドシェイク (またはブートストラップ過程の失敗) までを確認する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProvisionerTestResponse {
    /// 対象プロビジョナー名
    pub name: String,
    /// ハンドシェイクまで成功したか
    pub ok: bool,
    /// 一時ノードID (ハンドシェイク成功時)
    pub node_id: Option<String>,
    /// `stderr` に出力されたブートストラップログ (末尾 N 行)
    pub log_lines: Vec<String>,
    /// 失敗時のメッセージ
    pub error: Option<String>,
}

/// Git Worktree の情報 (`GET /api/v1/projects/:id/worktrees`)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorktreeInfo {
    /// ノードID
    pub node_id: String,
    /// Worktree のローカルパス
    pub path: String,
    /// チェックアウト中のブランチ
    pub branch: Option<String>,
    /// HEAD コミット
    pub head_commit: Option<String>,
    /// メインリポジトリ (Worktree ではない) か
    pub is_main: bool,
}

/// `GET /api/v1/projects/:id/worktrees` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorktreesResponse {
    /// Worktree 一覧
    pub worktrees: Vec<WorktreeInfo>,
}

/// `POST /api/v1/projects/:id/worktrees` リクエスト。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateWorktreeRequest {
    /// 対象ノードID
    pub node_id: String,
    /// 作成するブランチ名
    pub branch: String,
    /// 起点ブランチ (省略時は現在の HEAD)
    pub base_branch: Option<String>,
    /// 配置先パスの明示指定 (省略時は `worktree_dir_template` から解決)
    pub path: Option<String>,
}

/// `DELETE /api/v1/projects/:id/worktrees` リクエスト。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoveWorktreeRequest {
    /// 対象ノードID
    pub node_id: String,
    /// 削除対象パス
    pub path: String,
    /// 未コミット変更があっても強制削除するか
    pub force: bool,
}

/// `POST /api/v1/projects/:id/worktrees/prune` リクエスト。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PruneWorktreesRequest {
    /// 対象ノードID
    pub node_id: String,
}

/// `POST /api/v1/nodes/:node_id/projects/scan` リクエスト。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectScanRequest {
    /// スキャン対象ディレクトリ
    /// (省略時は `config.toml` の `node.project_scan_dirs` を走査)
    pub dir: Option<String>,
}

/// `POST /api/v1/nodes/:node_id/projects/scan` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectScanResponse {
    /// 実際に走査したディレクトリ
    pub scanned_dirs: Vec<String>,
    /// スキャン後のノード上での登録プロジェクト一覧
    pub projects: Vec<ProjectSummary>,
}

/// `POST /api/v1/nodes/:node_id/projects/link` リクエスト。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectLinkRequest {
    /// 紐付け先の論理プロジェクトID
    pub project_id: String,
    /// 対象ディレクトリ
    pub local_path: String,
}

/// `POST /api/v1/nodes/:node_id/projects/link` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectLinkResponse {
    /// 解決された論理プロジェクトID
    pub project_id: String,
    /// 対象ディレクトリ (解決後のローカルパス)
    pub local_path: String,
}

/// `GET /api/v1/sessions` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionListResponse {
    /// セッション一覧 (更新日時降順)
    pub sessions: Vec<SessionSummary>,
}

/// `GET /api/v1/agents` レスポンス (ACP Registry カタログ + 導入状態)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentsResponse {
    /// レジストリ (ビルトイン・カスタム含む) の全エージェント
    pub agents: Vec<AgentSummary>,
}

/// ACP エージェント 1 件 (カタログ情報 + 導入状態)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSummary {
    /// エージェントID
    pub id: String,
    /// 表示名
    pub name: String,
    /// レジストリ提供バージョン (ビルトイン・カスタムは `-`)
    pub version: String,
    /// 説明
    pub description: Option<String>,
    /// 配布形態 (`binary` / `npx` / `uvx` / `builtin` / `custom`)
    pub distributions: Vec<String>,
    /// 導入済みか (少なくとも 1 ノードに導入済み、または npx/uvx 配布)
    pub installed: bool,
    /// `config.toml` のカスタム定義か
    pub custom: bool,
    /// ビルトイン (`opencode2`) か
    pub builtin: bool,
    /// 導入済みバージョン一覧 (応答ノードのローカルキャッシュ)
    pub installed_versions: Vec<String>,
    /// 導入済みのノードID一覧 (NodeHello の報告ベース)
    pub installed_nodes: Vec<String>,
}

/// `POST /api/v1/nodes/:node_id/agents/update` リクエスト。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateAgentsRequest {
    /// 更新対象のエージェントID (省略時は導入済み全エージェント)
    pub agent_id: Option<String>,
}

/// エージェント管理操作 (install / update / remove) の共通レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentOpResponse {
    /// 実行結果の表示メッセージ (例: `installed opencode2 0.1.0`)
    pub message: Option<String>,
}

/// `GET /api/v1/nodes/tokens` の 1 件 (発行済みノード個別トークン)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeTokenSummary {
    /// 対象ノードID
    pub node_id: String,
    /// トークンハッシュの先頭プレフィックス (照合・表示用。平文は保存しない)
    pub token_prefix: String,
}

/// `GET /api/v1/nodes/tokens` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeTokensResponse {
    /// 発行済みノードトークン一覧
    pub tokens: Vec<NodeTokenSummary>,
}

/// `POST /api/v1/nodes/tokens` リクエスト (新規ノードトークン発行)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IssueNodeTokenRequest {
    /// ペアリングするノードID (fxg daemon の node_id)
    pub node_id: String,
}

/// `POST /api/v1/nodes/tokens` レスポンス。
///
/// 平文トークンは発行時にこの一度きりのみ返却される (サーバーには
/// SHA-256 ハッシュのみ保存)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IssueNodeTokenResponse {
    /// 対象ノードID
    pub node_id: String,
    /// 発行された平文トークン (`~/.flexagent/node_token` に保存する)
    pub token: String,
}

/// `POST /api/v1/auth/rotate-token` レスポンス (クライアントトークン再生成)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RotateAuthTokenResponse {
    /// 新しいクライアント認証トークン (旧トークンは即時無効化)
    pub token: String,
}

/// セッション起動時に同時作成する Worktree の指定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorktreeSpec {
    /// ブランチ名
    pub branch: String,
    /// 起点ブランチ
    pub base_branch: Option<String>,
    /// 配置先パスの明示指定
    pub new_path: Option<String>,
}

/// 別ノード・別エージェントへの Context Fork 指定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionForkSpec {
    /// 分岐元セッションID
    pub from_session_id: String,
    /// 分岐元のイベント連番 (省略時は最新)
    pub from_node_seq: Option<u64>,
    /// 一時VMから退避された Git バンドルを復元する場合の bundle データ (Base64)
    pub restore_git_bundle_b64: Option<String>,
}

/// `POST /api/v1/sessions` リクエスト (新規セッション開始)。
///
/// 既存の常駐 `node_id` 指定のほか、`provisioner` 指定による一時VMの
/// オンデマンド起動＆自動セットアップ、Worktree 同時作成、Context Fork に対応する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateSessionRequest {
    /// クライアント採番の相関ID (重複送信の冪等排除に使用)
    pub command_id: String,
    /// 論理プロジェクトID
    pub project_id: String,
    /// 実行する常駐ノードID (`provisioner` と排他)
    pub node_id: Option<String>,
    /// 一時VMプロビジョナー名 (`node_id` と排他。中央サーバー必須)
    pub provisioner: Option<String>,
    /// 実行ディレクトリ (既存 Worktree / リポジトリ直下を指定)
    pub local_path: Option<String>,
    /// 新規 Worktree を作成して起動する場合の指定
    pub worktree: Option<WorktreeSpec>,
    /// エージェントID (`opencode2` / `antigravity-acp` 等)
    pub agent_id: String,
    /// 初期プロンプト
    pub initial_prompt: Option<String>,
    /// 初期モード (`code` / `plan` 等)
    pub mode: Option<String>,
    /// OpenCode2 起動モード ("bridge" または "acp")
    pub opencode_mode: Option<String>,
    /// エージェントへの追加パススルー引数
    pub extra_args: Option<Vec<String>>,
    /// 既存セッションからの Fork 指定
    pub fork: Option<SessionForkSpec>,
}

/// `POST /api/v1/sessions` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateSessionResponse {
    /// 起動したセッションID
    pub session_id: String,
    /// 要求時の相関ID
    pub command_id: String,
}

/// `GET /api/v1/inbox` レスポンス (全セッション横断の未解決承認リクエスト)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InboxResponse {
    /// 未解決 (`pending`) の承認リクエスト一覧
    pub requests: Vec<PermissionRequestEntry>,
    /// 未解決 (`pending`) の elicitation (構造化入力) リクエスト一覧
    pub elicitations: Vec<ElicitationRequestEntry>,
}

/// `POST /api/v1/sessions/:id/permissions/:req_id/respond` リクエスト。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RespondPermissionRequest {
    /// 選択された `option_id` (`allow_once` / `allow_always` / `reject` 等)
    pub selected_option_id: String,
    /// 同一ツール操作をセッション中常時許可するか (`allow_always` 昇格)
    pub always: bool,
    /// 解決主体 (`cli` / `web` / `android_push`)
    pub resolved_by: String,
}

/// `POST /api/v1/sessions/:id/permissions/:req_id/respond` レスポンス。
///
/// 既に解決済みの場合も 200 で返し `already_resolved = true` とする
/// (UI 側は正常遷移として扱う。冪等)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RespondPermissionResponse {
    /// この応答より前に既に解決済みだったか
    pub already_resolved: bool,
}

/// `POST /api/v1/sessions/:id/elicitations/:elicitation_id/respond` リクエスト。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RespondElicitationRequest {
    /// ユーザーの応答アクション (`accept` / `decline` / `cancel`)
    pub action: ElicitationAction,
    /// `accept` 時の回答内容 (form の `requested_schema` 準拠。それ以外は `null`)
    pub content: serde_json::Value,
    /// 解決主体 (`cli` / `web` / `android_push`)
    pub resolved_by: String,
}

/// `POST /api/v1/sessions/:id/elicitations/:elicitation_id/respond` レスポンス。
///
/// 既に解決済みの場合も 200 で返し `already_resolved = true` とする (冪等)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RespondElicitationResponse {
    /// この応答より前に既に解決済みだったか
    pub already_resolved: bool,
}

/// `POST /api/v1/sessions/:id/revert` リクエスト。
///
/// 指定ターン (`UserMessage.node_seq`) 時点の Shadow Git Tree へ
/// ワークスペースを復元する (`fxg session revert` の Web UI 版)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionRevertRequest {
    /// Revert 基準にする `UserMessage` の `node_seq`
    /// (省略時は直近ターン)
    pub target_node_seq: Option<u64>,
}

/// `POST /api/v1/sessions/:id/revert` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionRevertResponse {
    /// 基準にした `UserMessage` の `node_seq`
    pub target_node_seq: u64,
    /// 復元先の Tree Hash
    pub restored_tree_hash: String,
    /// 復元直前を退避したバックアップ Tree Hash
    /// (もう一度 Revert すれば元に戻せる)
    pub backup_tree_hash: Option<String>,
    /// 復元したファイル数
    pub restored_files: u64,
    /// 削除したファイル数
    pub removed_files: u64,
}

/// `POST /api/v1/sessions/:id/resume` リクエスト。
///
/// 停止済みセッションを同一 `session_id` のまま再開する
/// (設計: `docs/04-agent-drivers-and-windows.md` §4.3)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResumeSessionRequest {
    /// ネイティブ復元を試みず履歴 Replay で継続する (既定 `false`)
    #[serde(default)]
    pub force_replay: bool,
}

/// `POST /api/v1/sessions/:id/resume` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResumeSessionResponse {
    /// 再開したセッションID (リクエスト対象と同一)
    pub session_id: String,
    /// エージェント側コンテキストをネイティブ復元できたか
    /// (`false` = 履歴 Replay で新規エージェントセッションとして継続)
    pub context_restored: bool,
}

/// `POST /api/v1/sessions/:id/archive` リクエスト。
///
/// アーカイブは一覧からの非表示/復元のみで、イベントログは保持される
/// (会話本文の消去は `DELETE /api/v1/sessions/:id`)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionArchiveRequest {
    /// `true` = アーカイブ、`false` = 復元
    pub archived: bool,
}

/// `POST /api/v1/sessions/:id/archive` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionArchiveResponse {
    /// 対象セッションID
    pub session_id: String,
    /// アーカイブ日時 (Unix epoch ms)。復元時は `None`
    pub archived_at: Option<i64>,
}

/// `POST /api/v1/search` レスポンス (FTS5 全文検索)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchResponse {
    /// 検索ヒット (新しい順)
    pub hits: Vec<SearchHit>,
}

/// `POST /api/v1/push/subscribe` リクエスト (PWA の Web Push 購読登録)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushSubscribeRequest {
    /// Push サービスエンドポイント URL
    pub endpoint: String,
    /// 購読の `p256dh` 公開鍵 (Base64)
    pub p256dh: String,
    /// 購読の `auth` シークレット (Base64)
    pub auth: String,
    /// 端末名 (例: `Pixel 9 Chrome PWA`)
    pub device_name: Option<String>,
}

/// `POST /api/v1/push/subscribe` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushSubscribeResponse {
    /// 登録に成功したか
    pub ok: bool,
}

/// `POST /api/v1/auth/login` リクエスト (Web UI のトークン入力)。
///
/// 認証済みリクエスト (`Authorization: Bearer <auth_token>`) として呼び出し、
/// 成功時は `Set-Cookie: fxg_session=<token>; HttpOnly; SameSite=Strict`
/// が返却される。以降は Cookie で認証できる (docs/03 §3.0)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuthLoginRequest {
    /// 認証トークン (Web UI が保持する値。再検証のため body でも渡す)
    pub token: String,
}

/// `POST /api/v1/system/kill-switch` リクエスト (緊急停止)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KillSwitchRequest {
    /// 実行理由 (監査ログへ記録)
    pub reason: String,
}

/// `POST /api/v1/system/kill-switch` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KillSwitchResponse {
    /// `KillAllSessions` を配信したノード数
    pub notified_nodes: u64,
}

/// `GET /api/v1/audit/logs` レスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditLogsResponse {
    /// 監査ログ (新しい順)
    pub logs: Vec<AuditLogEntry>,
}

/// Client → Server (`/api/v1/client/ws`) メッセージ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "snake_case")]
#[ts(export)]
pub enum ClientWsMessage {
    /// 接続時の購読開始 (差分再開カーソルの指定)。
    ///
    /// `since_cursor` は**接続先ストア**の `session_events.cursor` を指定する。
    /// 接続先を切り替えた場合は `None` (初回全量) から開始する。
    Subscribe {
        /// 最後に受信したバッチのカーソル
        since_cursor: Option<u64>,
        /// 現在開いているセッション (優先配信のヒント)
        focused_session_id: Option<String>,
    },
    /// プロンプト送信 (スラッシュコマンド含む)。
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
    /// 承認リクエストへの回答。
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
    /// elicitation (構造化入力リクエスト) への回答。
    RespondElicitation {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// ACP elicitation id
        elicitation_id: String,
        /// ユーザーの応答アクション (`accept` / `decline` / `cancel`)
        action: ElicitationAction,
        /// `accept` 時の回答内容 (form の `requested_schema` 準拠)
        content: serde_json::Value,
        /// 解決主体 (`cli` / `web` / `android_push`)
        resolved_by: String,
    },
    /// モード切替 / 設定変更 / キャンセル / Kill。
    ControlSession {
        /// 相関ID
        command_id: String,
        /// 対象セッションID
        session_id: String,
        /// 実行する操作
        action: SessionControlAction,
    },
    /// キープアライブ
    Ping,
}

/// Server → Client (`/api/v1/client/ws`) メッセージ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "snake_case")]
#[ts(export)]
pub enum ServerWsMessage {
    /// 差分イベントバッチ (接続直後のリプレイおよび以降のリアルタイム配信)。
    EventBatch {
        /// イベント列 (`node_seq` 昇順)
        events: Vec<crate::events::SessionEventEnvelope>,
        /// 配信元ストアのカーソル (再接続時に `since_cursor` へ渡す)
        cursor: u64,
    },
    /// ストリーミング途中のエフェメラルチャンク (永続化されない)。
    LiveStreamDelta {
        /// 対象セッションID
        session_id: String,
        /// 差分本体
        delta: StreamDeltaPayload,
    },
    /// 一時VMブートストラップ (`stderr`) の進捗ログ行 (エフェメラル)。
    ///
    /// ブートストラップは一時ノードの `node.db` が存在する前に発生するため、
    /// イベントログには永続化せず中央サーバーから直接ストリーム配信する。
    BootstrapLog {
        /// 対象セッションID
        session_id: String,
        /// ログ1行
        line: String,
    },
    /// コマンド実行結果 (要求元クライアントへの相関返却)。
    CommandResult {
        /// 相関ID
        command_id: String,
        /// 成功したか
        success: bool,
        /// 失敗時の構造化エラーコード
        code: Option<ErrorCode>,
        /// 人間向けメッセージ
        error: Option<String>,
        /// 対象セッションID (任意)
        session_id: Option<String>,
    },
    /// 同期状態の通知 (UI の同期バッジ表示用)。
    SystemStatus {
        /// 中央サーバーに接続中か (ローカルノード接続時)
        central_connected: bool,
        /// 未同期 Outbox イベント件数
        unsynced_event_count: u64,
        /// 最終同期日時 (Unix epoch ms)
        last_synced_at: Option<i64>,
    },
    /// エラー通知。
    Error {
        /// 構造化エラーコード
        code: ErrorCode,
        /// 人間向けメッセージ
        message: String,
    },
    /// `Ping` への応答
    Pong,
}

/// Client → Server (`/api/v1/pty/ws`) メッセージ。
///
/// Web UI / スマホPWA のターミナルとノード上の ConPTY / Unix PTY を直接結ぶ
/// 双方向ストリームチャネル。ノード側設定 `allow_remote_pty = false` の場合は
/// リモート (中央サーバー経由) からの `spawn` が拒否される。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "snake_case")]
#[ts(export)]
pub enum PtyClientMessage {
    /// 既存 PTY へのアタッチ
    Attach {
        /// PTY ID
        pty_id: String,
    },
    /// 新規 PTY の起動
    Spawn {
        /// 対象セッションID
        session_id: String,
        /// 列数
        cols: u16,
        /// 行数
        rows: u16,
        /// シェルコマンド (省略時は既定シェル)
        shell_cmd: Option<String>,
    },
    /// キー入力送信
    Input {
        /// 入力バイト列 (Base64)
        data_b64: String,
    },
    /// 画面リサイズ
    Resize {
        /// 列数
        cols: u16,
        /// 行数
        rows: u16,
    },
    /// PTY の終了
    Kill,
}

/// Server → Client (`/api/v1/pty/ws`) メッセージ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "snake_case")]
#[ts(export)]
pub enum PtyServerMessage {
    /// PTY 起動完了 (以降の出力は `attach` した PTY ID に紐づく)
    Spawned {
        /// 起動した PTY ID
        pty_id: String,
    },
    /// PTY 出力
    Output {
        /// 出力バイト列 (Base64)
        data_b64: String,
    },
    /// PTY プロセス終了
    Exit {
        /// 終了コード
        exit_code: Option<i32>,
    },
    /// エラー通知。
    ///
    /// `code` は `ErrorCode` ではなく PTY チャネル固有の文字列
    /// (例: リモートPTY無効時 `"FORBIDDEN"`)。
    Error {
        /// エラーコード文字列
        code: String,
        /// 人間向けメッセージ
        message: String,
    },
}

/// `CommandResult` を `ServerWsMessage::CommandResult` へ変換するヘルパー。
pub fn command_result_ws(result: CommandResult) -> ServerWsMessage {
    ServerWsMessage::CommandResult {
        command_id: result.command_id,
        success: result.success,
        code: result.code,
        error: result.error,
        session_id: result.session_id,
    }
}

/// `SessionEventBatch` を `ServerWsMessage::EventBatch` へ変換するヘルパー。
pub fn event_batch_ws(batch: SessionEventBatch) -> ServerWsMessage {
    ServerWsMessage::EventBatch {
        events: batch.events,
        cursor: batch.cursor,
    }
}

/// `GET /api/v1/nodes/:id/fs/browse` クエリパラメータ。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsBrowseQuery {
    /// 閲覧対象のディレクトリパス (省略時はホームディレクトリ)
    pub path: Option<String>,
}

/// ファイルシステム内のエントリ (フォルダまたはファイル)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsEntry {
    /// エントリ名 (ファイル名/フォルダ名)
    pub name: String,
    /// 絶対パス
    pub path: String,
    /// ディレクトリか
    pub is_dir: bool,
    /// 隠し属性 (名前が '.' で始まる、または Windows の Hidden 属性)
    pub is_hidden: bool,
}

/// `GET /api/v1/nodes/:id/fs/browse` レスポンス。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsBrowseResponse {
    /// 現在閲覧中の絶対パス (Windows では "C:\\Users\\..."、Unix では "/home/...")
    pub current_path: String,
    /// 親ディレクトリのパス (これ以上親がない、またはドライブ一覧なら None)
    pub parent_path: Option<String>,
    /// 子エントリ一覧
    pub entries: Vec<FsEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ws_message_tags() {
        let msg = ClientWsMessage::Subscribe {
            since_cursor: Some(10),
            focused_session_id: None,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "subscribe");
        assert_eq!(json["since_cursor"], 10);
        let decoded: ClientWsMessage = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);

        let ping = ClientWsMessage::Ping;
        assert_eq!(serde_json::to_value(&ping).unwrap()["op"], "ping");
    }

    #[test]
    fn server_ws_error_shape() {
        let msg = ServerWsMessage::Error {
            code: ErrorCode::NodeOffline,
            message: "node is offline".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "error");
        assert_eq!(json["code"], "NODE_OFFLINE");
    }

    #[test]
    fn api_error_response_shape() {
        let resp = ApiErrorResponse {
            error: ApiErrorBody {
                code: ErrorCode::AlreadyResolved,
                message: "already resolved".into(),
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "ALREADY_RESOLVED");
        assert_eq!(json["error"]["message"], "already resolved");
    }

    #[test]
    fn pty_messages_roundtrip() {
        let client = PtyClientMessage::Spawn {
            session_id: "s1".into(),
            cols: 80,
            rows: 24,
            shell_cmd: None,
        };
        let json = serde_json::to_value(&client).unwrap();
        assert_eq!(json["op"], "spawn");
        assert_eq!(
            serde_json::from_value::<PtyClientMessage>(json).unwrap(),
            client
        );

        let server = PtyServerMessage::Error {
            code: "FORBIDDEN".into(),
            message: "Remote PTY is disabled on this node by security policy".into(),
        };
        let json = serde_json::to_value(&server).unwrap();
        assert_eq!(json["op"], "error");
        assert_eq!(json["code"], "FORBIDDEN");
    }

    #[test]
    fn create_session_request_serialization() {
        let req = CreateSessionRequest {
            command_id: "c1".into(),
            project_id: "github.com/nazo6/flexagent".into(),
            node_id: Some("home-win".into()),
            provisioner: None,
            local_path: None,
            worktree: Some(WorktreeSpec {
                branch: "feat/auth".into(),
                base_branch: Some("main".into()),
                new_path: None,
            }),
            agent_id: "opencode2".into(),
            initial_prompt: Some("テストを修正して".into()),
            mode: Some("code".into()),
            opencode_mode: Some("bridge".into()),
            extra_args: Some(vec!["--verbose".into()]),
            fork: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["worktree"]["branch"], "feat/auth");
        assert!(json["provisioner"].is_null());
        let decoded: CreateSessionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, req);
    }
}
