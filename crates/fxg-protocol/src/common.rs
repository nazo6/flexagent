//! 共通補助型・エラーコード。
//!
//! `UnifiedEventPayload` および各プロトコルメッセージ・REST レスポンスから
//! 参照される型を定義する。すべてクライアント (UI / CLI) が扱うため
//! `#[derive(ts_rs::TS)]` + `#[ts(export)]` を付与する
//! (設計: `docs/03-protocol-and-api.md` §1.1)。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 列挙型の文字列表現パースに失敗したときのエラー。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} value: {value:?}")]
pub struct EnumParseError {
    /// 列挙型の種類名 (エラーメッセージ用)
    pub kind: &'static str,
    /// パースに失敗した文字列
    pub value: String,
}

impl EnumParseError {
    /// 新しいパースエラーを生成する。
    pub fn new(kind: &'static str, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }
}

/// セッション状態 (`sessions.status` と同値)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionStatus {
    /// 一時VMプロビジョニング中
    Provisioning,
    /// ブートストラップ (ツール自動構築) 中
    Bootstrapping,
    /// 待機中 (プロンプト入力待ち)
    Idle,
    /// ターン実行中
    Running,
    /// 権限承認待ち
    WaitingPermission,
    /// elicitation (質問/構造化入力) の回答待ち
    WaitingInput,
    /// 停止済み
    Stopped,
    /// エラー
    Error,
}

impl SessionStatus {
    /// DB / JSON 上の文字列表現。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Bootstrapping => "bootstrapping",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::WaitingPermission => "waiting_permission",
            Self::WaitingInput => "waiting_input",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SessionStatus {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "provisioning" => Ok(Self::Provisioning),
            "bootstrapping" => Ok(Self::Bootstrapping),
            "idle" => Ok(Self::Idle),
            "running" => Ok(Self::Running),
            "waiting_permission" => Ok(Self::WaitingPermission),
            "waiting_input" => Ok(Self::WaitingInput),
            "stopped" => Ok(Self::Stopped),
            "error" => Ok(Self::Error),
            other => Err(EnumParseError::new("session status", other)),
        }
    }
}

/// ノードのライフサイクル状態 (`nodes.lifecycle_status` と同値)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum NodeLifecycleStatus {
    /// プロビジョニング中 (一時VM)
    Provisioning,
    /// ブートストラップ中 (一時VM)
    Bootstrapping,
    /// 稼働可能
    Ready,
    /// 終了処理中 (Drain 中)
    Draining,
    /// 終了済み
    Terminated,
    /// エラー
    Error,
}

impl NodeLifecycleStatus {
    /// DB / JSON 上の文字列表現。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Bootstrapping => "bootstrapping",
            Self::Ready => "ready",
            Self::Draining => "draining",
            Self::Terminated => "terminated",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for NodeLifecycleStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for NodeLifecycleStatus {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "provisioning" => Ok(Self::Provisioning),
            "bootstrapping" => Ok(Self::Bootstrapping),
            "ready" => Ok(Self::Ready),
            "draining" => Ok(Self::Draining),
            "terminated" => Ok(Self::Terminated),
            "error" => Ok(Self::Error),
            other => Err(EnumParseError::new("node lifecycle status", other)),
        }
    }
}

/// 承認リクエストの状態 (`permission_requests.status` と同値)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PermissionRequestStatus {
    /// 未解決 (承認待ち)
    Pending,
    /// 承認済み
    Approved,
    /// 却下済み
    Rejected,
    /// キャンセル (セッション停止等)
    Cancelled,
}

impl PermissionRequestStatus {
    /// DB / JSON 上の文字列表現。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for PermissionRequestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PermissionRequestStatus {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(EnumParseError::new("permission request status", other)),
        }
    }
}

/// elicitation へのユーザー応答アクション (ACP `ElicitationAction` と同値)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ElicitationAction {
    /// 回答を送信する (form の入力内容を `content` に含める)
    Accept,
    /// 明示的に辞退する
    Decline,
    /// 操作を取り消す (ダイアログを閉じる等)
    Cancel,
}

impl ElicitationAction {
    /// JSON / IPC 上の文字列表現。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Decline => "decline",
            Self::Cancel => "cancel",
        }
    }
}

impl fmt::Display for ElicitationAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ElicitationAction {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "accept" => Ok(Self::Accept),
            "decline" => Ok(Self::Decline),
            "cancel" => Ok(Self::Cancel),
            other => Err(EnumParseError::new("elicitation action", other)),
        }
    }
}

/// elicitation リクエストの状態 (`elicitation_requests.status` と同値)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ElicitationRequestStatus {
    /// 未解決 (回答待ち)
    Pending,
    /// 回答済み (`accept`)
    Accepted,
    /// 辞退済み (`decline`)
    Declined,
    /// キャンセル (セッション停止等)
    Cancelled,
}

impl ElicitationRequestStatus {
    /// DB / JSON 上の文字列表現。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for ElicitationRequestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ElicitationRequestStatus {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "declined" => Ok(Self::Declined),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(EnumParseError::new("elicitation request status", other)),
        }
    }
}

/// 論理プロジェクト (`project_key`) の解決元。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProjectResolutionSource {
    /// `.fxg.toml` の明示指定 (最優先)
    FxgToml,
    /// `git remote.origin.url` の正規化
    GitRemote,
    /// フォールバック (`local:<node_id>:<path-hash>`)
    Fallback,
}

impl ProjectResolutionSource {
    /// 表示用の識別子。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FxgToml => "fxg_toml",
            Self::GitRemote => "git_remote",
            Self::Fallback => "fallback",
        }
    }
}

impl fmt::Display for ProjectResolutionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProjectResolutionSource {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fxg_toml" => Ok(Self::FxgToml),
            "git_remote" => Ok(Self::GitRemote),
            "fallback" => Ok(Self::Fallback),
            other => Err(EnumParseError::new("project resolution source", other)),
        }
    }
}

/// Worktree 作成時の `copy_files` / `post_create` フック実行ログ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HookLogEntry {
    /// 実行したコマンド文字列
    pub command: String,
    /// 成功したか
    pub success: bool,
    /// 出力 (stdout + stderr)
    pub output: String,
}

/// API レスポンス用のセッション集約型 (`sessions` 行から構成)。
///
/// ハブ側ではイベント適用による投影から生成される。Local Node と
/// Central Server の双方が同一の型を返却する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionSummary {
    /// セッションID (UUID v7)
    pub session_id: String,
    /// 論理プロジェクトID (例: `github.com/nazo6/flexagent`)
    pub project_id: String,
    /// 実行ノードID
    pub node_id: String,
    /// 実行ディレクトリ (Worktree パス含む)
    pub local_path: String,
    /// 起動時点のスナップショット (ライブ値は Worktree API から取得)
    pub git_branch: Option<String>,
    /// Worktree 内での実行か
    pub is_worktree: bool,
    /// エージェントID (`opencode2` / `antigravity-acp` 等)
    pub agent_id: String,
    /// エージェント内部のセッションID (`SessionAgentBound` イベントで確定)
    pub agent_session_id: Option<String>,
    /// Fork元のセッションID
    pub parent_session_id: Option<String>,
    /// 親セッションのどのイベント (`node_seq`) 時点から Fork / Revert したか
    pub fork_from_node_seq: Option<u64>,
    /// セッションタイトル
    pub title: String,
    /// セッション状態
    pub status: SessionStatus,
    /// 現在のACR SessionMode (例: `code`, `plan`)
    pub current_mode: Option<String>,
    /// 永続化済み最新の `node_seq` (ノード) / 投影に適用済みの最大 `node_seq` (ハブ)
    pub last_node_seq: u64,
    /// 作成日時 (Unix epoch ms)
    pub created_at: i64,
    /// 更新日時 (Unix epoch ms)
    pub updated_at: i64,
    /// アーカイブ日時 (Unix epoch ms)。`None` = 未アーカイブ。
    ///
    /// アーカイブ済みセッションは既定の一覧から除外される
    /// (`include_archived` 指定時のみ返却)。
    pub archived_at: Option<i64>,
}

/// `NodeHello` で報告するプロジェクト紐付け情報。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeProjectReport {
    /// 論理プロジェクトID
    pub project_id: String,
    /// 表示名
    pub name: String,
    /// 正規化元Git URL
    pub canonical_git_url: Option<String>,
    /// このノード上のローカルパス紐付け (Worktree 含む)
    pub bindings: Vec<ProjectNodeBinding>,
}

/// プロジェクト × ノードのローカルパス紐付け。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectNodeBinding {
    /// ノード上のローカルパス
    pub local_path: String,
    /// Git Worktree か (false はメインリポジトリ)
    pub is_worktree: bool,
    /// 報告時点でノード上に実在するか (ディレクトリ確認)
    pub path_exists: bool,
    /// 最終確認時のGitブランチ
    pub git_branch: Option<String>,
    /// 最終使用日時 (Unix epoch ms)
    pub last_used_at: i64,
}

/// ストリーミング途中のエフェメラルチャンク。
///
/// DB へは永続化されず、再送もされない (`LiveStreamDelta` 専用)。
/// 完成イベント (`AgentMessage` 等) 到着時にクライアント側で確定表示へ置き換える。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "delta_type", rename_all = "snake_case")]
#[ts(export)]
pub enum StreamDeltaPayload {
    /// エージェント返答メッセージの差分テキスト
    AgentMessageDelta {
        /// メッセージID (`AgentMessage.message_id` と対応)
        message_id: String,
        /// 追加テキスト
        text_delta: String,
    },
    /// 思考プロセスの差分テキスト
    AgentThoughtDelta {
        /// 思考ID (`AgentThought.thought_id` と対応)
        thought_id: String,
        /// 追加テキスト
        text_delta: String,
    },
    /// ツール呼び出しの進捗更新
    ToolCallProgress {
        /// ツール呼び出しID
        tool_call_id: String,
        /// 更新後ステータス
        status: String,
        /// 出力差分 (任意)
        raw_output_delta: Option<String>,
    },
    /// ACP terminal/* の出力差分
    TerminalOutputDelta {
        /// ターミナルID
        terminal_id: String,
        /// 出力バイト列 (Base64)
        data_b64: String,
    },
}

/// ユーザーメッセージに添付されるファイルのメタデータ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AttachmentMeta {
    /// ファイル名
    pub file_name: String,
    /// MIMEタイプ
    pub mime_type: String,
    /// サイズ (bytes)
    pub size_bytes: u64,
    /// ノード上の実ファイル参照
    pub local_path: Option<String>,
    /// 小さい添付のインライン転送用 (Base64)
    pub data_b64: Option<String>,
}

/// ファイル変更の Unified Diff 情報。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileDiff {
    /// 対象ファイルパス
    pub path: String,
    /// 変更前テキスト
    pub old_text: Option<String>,
    /// 変更後テキスト
    pub new_text: Option<String>,
    /// Unified Diff 本文
    pub unified_diff: String,
    /// 追加行数
    pub additions: u32,
    /// 削除行数
    pub deletions: u32,
}

/// エージェントの実行計画エントリ (ACP Plan)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanEntry {
    /// エントリID
    pub id: String,
    /// 表示タイトル
    pub title: String,
    /// ステータス (`pending` / `in_progress` / `completed` 等)
    pub status: String,
}

/// 権限承認の選択肢 (ACP `PermissionOption`)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermissionOption {
    /// 選択肢ID (`PermissionResolved.selected_option_id` に指定する値)
    pub option_id: String,
    /// 表示名
    pub name: String,
    /// 種別 (`allow_once` / `allow_always` / `reject_once` / `reject_always` 等)
    pub kind: String,
}

impl PermissionOption {
    /// 却下系の選択肢か (種別が `reject` で始まる)。
    pub fn is_reject_kind(kind: &str) -> bool {
        kind.starts_with("reject")
    }
}

/// ACP SessionMode の情報。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModeInfo {
    /// モードID (例: `code`, `plan`)
    pub mode_id: String,
    /// 表示名
    pub name: String,
    /// 説明
    pub description: Option<String>,
}

/// ACP スラッシュコマンドの情報。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommandInfo {
    /// コマンド名
    pub name: String,
    /// 説明
    pub description: String,
    /// 入力ヒント
    pub input_hint: Option<String>,
}

/// エージェント設定項目 (モデル選択等) の情報。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConfigOptionInfo {
    /// 設定キー
    pub key: String,
    /// 表示名
    pub name: String,
    /// 現在値
    pub current_value: serde_json::Value,
    /// 選択可能な値の一覧
    pub options: Vec<serde_json::Value>,
}

/// セッション制御アクション (`ControlSession` コマンド)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export)]
pub enum SessionControlAction {
    /// モード切替 (`plan` / `code` 等)
    SetMode {
        /// 切替先モードID
        mode_id: String,
    },
    /// 設定変更 (モデル選択等)
    SetConfig {
        /// 設定キー
        key: String,
        /// 新しい値
        value: serde_json::Value,
    },
    /// 現在のターンを中断
    Cancel,
    /// セッションの完全終了
    Kill,
}

/// Worktree 操作アクション (`ManageWorktree` コマンド)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export)]
pub enum WorktreeAction {
    /// 新規 Worktree 作成 (`git worktree add -b <branch> <path> <base>`)
    Add {
        /// 作成するブランチ名
        branch: String,
        /// 起点ブランチ (省略時は現在の HEAD)
        base_branch: Option<String>,
        /// 配置先パスの明示指定 (省略時は `worktree_dir_template` から解決)
        new_path: Option<String>,
    },
    /// Worktree 削除 (`git worktree remove`)
    Remove {
        /// 削除対象パス
        path: String,
        /// 未コミット変更があっても強制削除するか
        force: bool,
    },
    /// 削除済み Worktree の管理情報をクリーンアップ (`git worktree prune`)
    Prune,
}

/// エージェント管理アクション (`ManageAgent` コマンド / Client API 共通)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export)]
pub enum AgentAction {
    /// ACP Registry からダウンロード・展開して導入 (`fxg agents install`)
    Install {
        /// エージェントID (エイリアス可)
        agent_id: String,
    },
    /// 導入済みエージェントを更新 (`fxg agents update`)
    Update {
        /// 更新対象のエージェントID (省略時は導入済み全エージェント)
        agent_id: Option<String>,
    },
    /// キャッシュ済みバイナリを削除 (`fxg agents remove`)
    Remove {
        /// エージェントID (エイリアス可)
        agent_id: String,
    },
}

/// Diff のスコープ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiffScope {
    /// 作業ツリーの未コミット差分 (`git diff HEAD`)
    Uncommitted,
    /// ベースブランチとの累積差分 (`git diff <base>...HEAD`)
    BranchBase,
}

/// Git 作業ツリーの Diff 取得結果 (`GET /api/v1/sessions/:id/diff`)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceDiffResponse {
    /// 取得した Diff のスコープ
    pub scope: DiffScope,
    /// 比較対象のベースブランチ (`BranchBase` 時)
    pub base_branch: Option<String>,
    /// 現在の HEAD コミット
    pub head_commit: String,
    /// 変更ファイルごとの Diff
    pub files: Vec<FileDiff>,
}

/// セッション Fork 時に履歴注入するための会話履歴アイテム。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForkHistoryItem {
    /// 発話ロール (`user` / `agent` 等)
    pub role: String,
    /// 本文
    pub text: String,
    /// ツール呼び出し等の要約 (任意)
    pub tool_summary: Option<String>,
}

/// 共通エラーコード (`CommandResult` および REST エラーレスポンス双方で使用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum ErrorCode {
    /// 認証トークンが無い・一致しない (HTTP `401 Unauthorized`)
    Unauthorized,
    /// `Host` / `Origin` 検証失敗・ポリシー違反 (HTTP `403 Forbidden`。
    /// リモートPTY無効時の `PTY_DISABLED` とは区別する)
    Forbidden,
    /// 対象ノードが中央サーバーから切断中 (キューイングせず即時返却)
    NodeOffline,
    /// 承認リクエストが既に解決済み (2回目以降の応答。冪等に扱う)
    AlreadyResolved,
    /// セッションが実行中 (busy)
    Busy,
    /// 現在の状態では実行できない操作
    InvalidState,
    /// 停止済みセッションへの操作だが、エージェントがネイティブ復元
    /// (`session/resume` 等) に対応していない。履歴 Replay での明示的な
    /// 再開 (`fxg session resume` / Web UI の Resume) が必要。
    ResumeRequired,
    /// リモートPTYがセキュリティポリシーで無効化されている
    PtyDisabled,
    /// `command_id` の重複送信 (ダブルタップ・WS再送)
    CommandDuplicate,
    /// 対象が見つからない
    NotFound,
    /// 内部エラー
    Internal,
}

impl ErrorCode {
    /// JSON / WS / REST 上の文字列表現。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Forbidden => "FORBIDDEN",
            Self::NodeOffline => "NODE_OFFLINE",
            Self::AlreadyResolved => "ALREADY_RESOLVED",
            Self::Busy => "BUSY",
            Self::InvalidState => "INVALID_STATE",
            Self::ResumeRequired => "RESUME_REQUIRED",
            Self::PtyDisabled => "PTY_DISABLED",
            Self::CommandDuplicate => "COMMAND_DUPLICATE",
            Self::NotFound => "NOT_FOUND",
            Self::Internal => "INTERNAL",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ErrorCode {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "UNAUTHORIZED" => Ok(Self::Unauthorized),
            "FORBIDDEN" => Ok(Self::Forbidden),
            "NODE_OFFLINE" => Ok(Self::NodeOffline),
            "ALREADY_RESOLVED" => Ok(Self::AlreadyResolved),
            "BUSY" => Ok(Self::Busy),
            "INVALID_STATE" => Ok(Self::InvalidState),
            "RESUME_REQUIRED" => Ok(Self::ResumeRequired),
            "PTY_DISABLED" => Ok(Self::PtyDisabled),
            "COMMAND_DUPLICATE" => Ok(Self::CommandDuplicate),
            "NOT_FOUND" => Ok(Self::NotFound),
            "INTERNAL" => Ok(Self::Internal),
            other => Err(EnumParseError::new("error code", other)),
        }
    }
}

/// コマンド実行結果 (Node → Server → 要求元クライアントへ相関返却)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommandResult {
    /// `ServerToNodeMsg` の各コマンドに付与された相関ID
    pub command_id: String,
    /// 成功したか
    pub success: bool,
    /// 失敗時の構造化エラーコード
    pub code: Option<ErrorCode>,
    /// 人間向けエラーメッセージ
    pub error: Option<String>,
    /// 対象セッションID (任意)
    pub session_id: Option<String>,
}

/// クライアント向けプロジェクト投影 (`GET /api/v1/projects`)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectSummary {
    /// 論理プロジェクトID
    pub project_id: String,
    /// 表示名
    pub name: String,
    /// 正規化元Git URL
    pub canonical_git_url: Option<String>,
    /// 作成日時 (Unix epoch ms)
    pub created_at: i64,
    /// 更新日時 (Unix epoch ms)
    pub updated_at: i64,
    /// 参加ノードごとのローカルパス紐付け
    pub bindings: Vec<ProjectBindingSummary>,
}

/// クライアント向けプロジェクト × ノード紐付け。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectBindingSummary {
    /// ノードID
    pub node_id: String,
    /// ノード上のローカルパス
    pub local_path: String,
    /// Git Worktree か
    pub is_worktree: bool,
    /// ローカルパスがノード上に実在するか
    ///
    /// 中央サーバーでは最後の NodeHello 報告時点、ローカルノードでは
    /// 取得時にライブ確認した値。
    pub path_exists: bool,
    /// 最終確認時のGitブランチ
    pub git_branch: Option<String>,
    /// 最終使用日時 (Unix epoch ms)
    pub last_used_at: i64,
}

/// クライアント向けノード情報 (`GET /api/v1/nodes` / `GET /api/v1/system/info`)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeSummary {
    /// ノードID
    pub node_id: String,
    /// 表示名
    pub name: String,
    /// OS (`windows` / `linux` / `macos`)
    pub os: String,
    /// アーキテクチャ (`x86_64` / `aarch64`)
    pub arch: String,
    /// fxg バイナリバージョン
    pub version: String,
    /// 一時VM / コンテナノードか
    pub is_ephemeral: bool,
    /// 一時ノードのプロビジョナー識別子
    pub provisioner: Option<String>,
    /// ライフサイクル状態
    pub lifecycle_status: NodeLifecycleStatus,
    /// 接続中か
    pub is_online: bool,
    /// 最終疎通日時 (Unix epoch ms)
    pub last_seen_at: i64,
    /// 利用可能なエージェントID一覧
    pub installed_agents: Vec<String>,
}

/// クライアント向け承認リクエスト投影 (グローバル承認 Inbox)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermissionRequestEntry {
    /// ACP request id
    pub request_id: String,
    /// 対象セッションID
    pub session_id: String,
    /// 対象ノードID
    pub node_id: String,
    /// ツール名 (例: `terminal/create`)
    pub tool_name: String,
    /// 要約 (例: `Run command: cargo test`)
    pub summary: String,
    /// 選択肢一覧
    pub options: Vec<PermissionOption>,
    /// 引数や Diff 詳細
    pub details: serde_json::Value,
    /// 状態
    pub status: PermissionRequestStatus,
    /// 作成日時 (Unix epoch ms)
    pub created_at: i64,
    /// 解決日時 (Unix epoch ms)
    pub resolved_at: Option<i64>,
    /// 解決主体 (`cli` / `web` / `android_push`)
    pub resolved_by: Option<String>,
}

/// クライアント向け elicitation リクエスト投影 (質問 Inbox)。
///
/// エージェントが `elicitation/create` でユーザーへ構造化入力を求めた
/// リクエストを表す (Phase 1 は form モードのみ)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ElicitationRequestEntry {
    /// ACP elicitation id
    pub elicitation_id: String,
    /// 対象セッションID
    pub session_id: String,
    /// 対象ノードID
    pub node_id: String,
    /// ユーザーへ提示するメッセージ
    pub message: String,
    /// 要求モード (`form` / `url`。Phase 1 は `form` のみ)
    pub mode: String,
    /// form モードの要求 JSON Schema (`requestedSchema`)
    pub requested_schema: serde_json::Value,
    /// 関連するツール呼び出しID (任意)
    pub tool_call_id: Option<String>,
    /// `accept` 時の回答内容 (未回答・decline / cancel では `Value::Null`)
    pub content: serde_json::Value,
    /// 状態
    pub status: ElicitationRequestStatus,
    /// 作成日時 (Unix epoch ms)
    pub created_at: i64,
    /// 解決日時 (Unix epoch ms)
    pub resolved_at: Option<i64>,
    /// 解決主体 (`cli` / `web` / `android_push`)
    pub resolved_by: Option<String>,
}

/// FTS5 全文検索のヒット。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchHit {
    /// 対象セッションID
    pub session_id: String,
    /// 対象イベントID
    pub event_id: String,
    /// セッション内連番
    pub node_seq: u64,
    /// イベント種別
    pub event_type: String,
    /// スニペット (マッチ箇所を含む抜粋)
    pub snippet: String,
    /// イベント作成日時 (Unix epoch ms)
    pub created_at: i64,
}

/// 監査ログエントリ (`GET /api/v1/audit/logs`)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditLogEntry {
    /// ログID
    pub id: i64,
    /// 操作種別 (`session_start` / `permission_resolved` / `pty_spawn` / `kill_switch` / `worktree_manage`)
    pub action: String,
    /// 関連セッションID (任意)
    pub session_id: Option<String>,
    /// 対象ノードID (任意)
    pub node_id: Option<String>,
    /// 送信元IPアドレス
    pub client_ip: String,
    /// クライアントUser-Agent
    pub client_user_agent: Option<String>,
    /// トークン識別子または認証主体
    pub auth_subject: String,
    /// 実行内容詳細
    pub details: serde_json::Value,
    /// 記録日時 (Unix epoch ms)
    pub created_at: i64,
}
