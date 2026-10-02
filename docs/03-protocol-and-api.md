# 03. プロトコル・API・IPC仕様 (`fxg-protocol`)

`crates/fxg-protocol` クレートに定義する共通データ型と、3つの通信経路（① Node ⇔
Server WebSocket、② Client ⇔ Server/Node HTTP+WS、③ CLI ⇔ Node Local
IPC）のプロトコル仕様です。 クライアント (UI / CLI) が直接扱う型には
`#[derive(Serialize, Deserialize, ts_rs::TS)]` + `#[ts(export)]`
を付与し、`ui/src/lib/generated/`（SvelteKit の `$lib/generated/`）
へTypeScriptの型定義を自動出力します（設定ファイルスキーマ等のサーバー内部型は
`ts-rs` export 対象外とする）。

---

## 1. 正規化セッションイベント型 (`UnifiedEventPayload`)

ACP (`agent-client-protocol-schema`)
のイベントモデルをベースに、ターミナル出力や承認解決イベントを統合した型です。

```rust
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionEventEnvelope {
    pub event_id: String,       // UUID v7 (冪等適用・重複排除)
    pub session_id: String,     // UUID v7
    pub node_seq: u64,          // セッション内連番 (実行ノードのみが採番。順序の正)
    pub created_at: i64,        // Unix epoch ms
    pub payload: UnifiedEventPayload,
}

/// カーソルを伴うイベントバッチ (Client WS 配信 / 履歴 API の共通型)
/// cursor は「配信元ストア (node.db / server.db) への取り込み順」。
/// クライアントは最後に受信したバッチの cursor を保存し、再接続時に
/// Subscribe { since_cursor } へ渡す (接続先ストア以外の cursor は意味を持たない)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionEventBatch {
    pub events: Vec<SessionEventEnvelope>,
    pub cursor: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[ts(export)]
pub enum UnifiedEventPayload {
    /// セッション生成直後の最初のイベント (node_seq = 1)。
    /// ハブ側 sessions 投影の生成源 (別系統のメタデータ同期は行わない)
    SessionCreated {
        /// 実行ノードID。イベントは接続元ノードから送られるため transport 上は
        /// 暗黙に決まるが、payload に含めることでイベントログのみから
        /// ハブ側 sessions 投影を完全再構築できる (NodeHello 再送・Resync /
        /// ハブDB再構築時の復元を保証する)
        node_id: String,
        project_id: String,
        project_name: String,
        local_path: String,
        git_branch: Option<String>,      // 起動時点のスナップショット
        is_worktree: bool,
        agent_id: String,
        parent_session_id: Option<String>,
        fork_from_node_seq: Option<u64>,
        title: String,
    },
    /// セッションタイトル変更 (UI/CLI からのリネーム。コマンドとして実行ノードに到達してから発行される)
    SessionTitleChanged {
        title: String,
    },
    /// ACP セッション確立などによる agent_session_id の確定
    SessionAgentBound {
        agent_session_id: String,
    },
    /// ユーザーが送信したプロンプト（スラッシュコマンド含む）
    UserMessage {
        text: String,
        attachments: Vec<AttachmentMeta>,
        client_source: String,        // "cli" | "web" | "android"
        /// ターン開始直前の Shadow Git Tree Hash (Revert用。snapshot_enabled=false 時は None。
        /// この payload が唯一の正であり、専用DBカラムへの複製は行わない)
        snapshot_tree_hash: Option<String>,
    },
    /// エージェントの返答メッセージ（ターン完了時に is_complete=true の完成イベントのみ永続化）
    /// ※ストリーミング途中は LiveStreamDelta として配信され、永続化されない
    AgentMessage {
        message_id: String,
        text: String,
        is_complete: bool,
    },
    /// エージェントの思考プロセス (Thinking)
    /// ※AgentMessage と同様、ターン完了時に is_complete=true の完成イベントのみ永続化
    AgentThought {
        thought_id: String,
        text: String,
        is_complete: bool,
    },
    /// ツール呼び出しとファイルDiff等の状態
    /// ※同一 tool_call_id のイベント追記によって状態更新を表現する (tool_update イベントは廃止)
    ToolCall {
        tool_call_id: String,
        title: String,
        kind: String,          // "read" | "edit" | "execute" | "search" | "other"
        status: String,        // "pending" | "in_progress" | "completed" | "failed"
        locations: Vec<String>,// 対象ファイルパス
        diff: Option<FileDiff>,// ファイル変更時のUnified DiffまたはBefore/After
        raw_output: Option<String>,
    },
    /// エージェントの実行計画 (ACP Plan)
    PlanUpdate {
        entries: Vec<PlanEntry>,
    },
    /// エージェントからの権限承認リクエスト (ACP session/request_permission)
    PermissionRequest {
        request_id: String,
        tool_name: String,
        summary: String,
        options: Vec<PermissionOption>, // 例: "allow_once", "allow_always", "reject"
        details: serde_json::Value,
    },
    /// 承認リクエストの解決結果
    PermissionResolved {
        request_id: String,
        selected_option_id: String,
        resolved_by: String,   // "cli" | "web" | "android_push"
    },
    /// エージェントからの構造化入力リクエスト (ACP elicitation/create。Phase 1 は form モード)
    /// requested_schema から UI が回答フォームを生成する
    ElicitationRequest {
        elicitation_id: String,       // form は JSON-RPC request id
        message: String,
        mode: String,                 // "form" | "url" (Phase 1 は "form" のみ)
        requested_schema: serde_json::Value,
        tool_call_id: Option<String>,
    },
    /// 構造化入力リクエストの解決結果
    ElicitationResolved {
        elicitation_id: String,
        action: ElicitationAction,    // "accept" | "decline" | "cancel"
        content: serde_json::Value,   // accept 時の回答内容 (decline/cancel は null)
        resolved_by: String,          // "cli" | "web" | "android_push" | "system"
    },
    /// ACP terminal/* または PTY の出力チャンク (永続化対象。ただし FTS の searchable_text からは除外)
    TerminalOutput {
        terminal_id: String,
        command: String,
        data_b64: String,      // ANSIエスケープを含む生バイト列 (Base64)
        exit_code: Option<i32>,
    },
    /// 対話型コマンドへの標準入力送信 (Web UI -> エージェントPTY)
    /// ※入力キーストロークはエフェメラル扱いで、イベントログには永続化しない (PTY WS 経由の生配信も可)
    TerminalInput {
        terminal_id: String,
        data_b64: String,      // ユーザー入力キーストローク (Base64)
    },
    /// モード・スラッシュコマンド・設定の更新通知 (永続化対象: セッション復元時に使用)
    /// ハブ側 sessions.current_mode / available_modes_json /
    /// available_commands_json / config_options_json 投影の更新源
    CapabilitiesUpdated {
        current_mode: Option<String>,
        available_modes: Vec<ModeInfo>,
        available_commands: Vec<CommandInfo>,
        config_options: Vec<ConfigOptionInfo>,
    },
    /// 一時VMプロビジョニング・自動ツール構築 (stderr) の進捗ログ行 (永続化対象)
    BootstrapLog {
        line: String,
    },
    /// コンテキスト使用量・累積コストの更新 (ACP usage_update)
    /// ハブ側 sessions.usage_json 投影の更新源 (累積ではなく最新の値を保持)
    UsageUpdated {
        used_tokens: u64,        // 現在コンテキストに含まれるトークン数
        context_size: u64,       // コンテキストウィンドウ全体のサイズ
        cost: Option<UsageCost>, // 累積コスト (エージェントが報告した場合)
    },
    /// ターンが正常終了以外の理由で打ち切られた (ACP StopReason)
    /// EndTurn / Cancelled は正常終了としてイベント化しない。
    /// UI / CLI はタイムラインにシステム行として表示する
    TurnEnded {
        reason: TurnStopReason,  // "max_tokens" | "max_turn_requests" | "refusal"
        message: Option<String>,
    },
    /// コンテキスト圧縮 (compaction) の進行状況 (永続化対象)
    CompactionUpdated {
        status: CompactionStatus, // "started" | "completed" | "failed"
        detail: Option<String>,   // 失敗理由などの補足 (任意)
    },
    /// セッション状態の変化 ('provisioning' | 'bootstrapping' | 'idle' | 'running' | ...)
    /// ハブ側 sessions.status 投影の更新源
    StatusChanged {
        status: SessionStatus,
        error_message: Option<String>,
    },
    /// セッションのアーカイブ状態変更 (可逆な可視性フラグ)
    /// ハブ側 sessions.archived_at 投影の更新源。アーカイブ済みセッションは
    /// 既定の一覧から除外される (include_archived 指定時のみ返却)
    SessionArchived {
        archived: bool,        // true = アーカイブ, false = 復元
    },
    /// セッションの削除 (tombstone)
    /// 適用時に本イベントより前のイベント本文と permission_requests /
    /// elicitation_requests をパージする。
    /// 本イベント (tombstone) と sessions 行は Outbox / Resync での削除伝播と
    /// Fork 元参照の整合のため残す (復元不能)
    SessionDeleted {},
}
```

### 1.1 共通補助型・エラーコード

`UnifiedEventPayload` および各プロトコルメッセージから参照される補助型です。
すべてクライアントが扱うため `#[derive(Serialize, Deserialize, ts_rs::TS)]` +
`#[ts(export)]` を付与します。

```rust
/// API レスポンス用のセッション集約型（sessions 行から構成。
/// ハブ側ではイベント適用による投影から生成される）
pub struct SessionSummary {
    pub session_id: String,
    pub project_id: String,
    pub node_id: String,
    pub local_path: String,
    pub git_branch: Option<String>,
    pub is_worktree: bool,
    pub agent_id: String,
    pub agent_session_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub fork_from_node_seq: Option<u64>,
    pub title: String,
    pub status: SessionStatus,
    pub current_mode: Option<String>,
    /// 最新のコンテキスト使用量・累積コスト (UsageUpdated の投影。None = 未受信)
    pub usage: Option<SessionUsage>,
    pub last_node_seq: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// セッションのコンテキスト使用量・累積コスト (ACP usage_update)
pub struct SessionUsage {
    pub used_tokens: u64,
    pub context_size: u64,
    pub cost: Option<UsageCost>,
}

/// セッションの累積コスト (ACP Cost)
pub struct UsageCost {
    pub amount: f64,
    pub currency: String, // ISO 4217 (例: "USD")
}

/// ターン打ち切り理由のうち、ユーザーへ提示すべきもの (ACP StopReason)
pub enum TurnStopReason {
    MaxTokens,        // 最大トークン数
    MaxTurnRequests,  // 1ターン内の要求回数上限
    Refusal,          // エージェントが継続を拒否
}

/// コンテキスト圧縮 (compaction) の進行状態 (CompactionUpdated の状態表現)
pub enum CompactionStatus {
    Started,    // 圧縮を開始した
    Completed,  // 圧縮が完了した
    Failed,     // 圧縮に失敗した
}

/// NodeHello で報告するプロジェクト紐付け情報
pub struct NodeProjectReport {
    pub project_id: String,
    pub name: String,
    pub canonical_git_url: Option<String>,
    pub bindings: Vec<ProjectNodeBinding>,
}

pub struct ProjectNodeBinding {
    pub local_path: String,
    pub is_worktree: bool,
    pub path_exists: bool, // 報告時点でノード上に実在するか (ディレクトリ確認)
    pub git_branch: Option<String>,
    pub last_used_at: i64,
}

/// ストリーミング途中のエフェメラルチャンク (再送されず、永続化もされない)
#[serde(tag = "delta_type", rename_all = "snake_case")]
pub enum StreamDeltaPayload {
    AgentMessageDelta { message_id: String, text_delta: String },
    AgentThoughtDelta { thought_id: String, text_delta: String },
    ToolCallProgress { tool_call_id: String, status: String, raw_output_delta: Option<String> },
    TerminalOutputDelta { terminal_id: String, data_b64: String },
}

pub struct AttachmentMeta {
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub local_path: Option<String>, // ノード上の実ファイル参照
    pub data_b64: Option<String>,   // 小さい添付のインライン転送用
}

pub struct FileDiff {
    pub path: String,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
    pub unified_diff: String,
    pub additions: u32,
    pub deletions: u32,
}

pub struct PlanEntry { pub id: String, pub title: String, pub status: String }

pub struct PermissionOption { pub option_id: String, pub name: String, pub kind: String }

pub struct ModeInfo { pub mode_id: String, pub name: String, pub description: Option<String> }

pub struct CommandInfo { pub name: String, pub description: String, pub input_hint: Option<String> }

pub struct ConfigOptionInfo {
    pub key: String,
    pub name: String,
    pub current_value: serde_json::Value,
    pub options: Vec<serde_json::Value>,
}

#[serde(rename_all = "snake_case")]
pub enum SessionStatus { Provisioning, Bootstrapping, Idle, Running, WaitingPermission, WaitingInput, Stopped, Error }

#[serde(tag = "action", rename_all = "snake_case")]
pub enum SessionControlAction {
    SetMode { mode_id: String },
    SetConfig { key: String, value: serde_json::Value },
    Cancel,
    Compact,  // コンテキスト圧縮 (要約)。対応ドライバのみ (既定は未対応エラー)
    Kill,
}

#[serde(tag = "action", rename_all = "snake_case")]
pub enum WorktreeAction {
    Add { branch: String, base_branch: Option<String>, new_path: Option<String> },
    Remove { path: String, force: bool },
}

#[serde(rename_all = "snake_case")]
pub enum DiffScope { Uncommitted, BranchBase }

pub struct WorkspaceDiffResponse {
    pub scope: DiffScope,
    pub base_branch: Option<String>,
    pub head_commit: String,
    pub files: Vec<FileDiff>,
}

pub struct ForkHistoryItem { pub role: String, pub text: String, pub tool_summary: Option<String> }

/// 共通エラーコード (CommandResult および REST エラーレスポンス双方で使用)
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    NodeOffline,
    AlreadyResolved,
    Busy,
    InvalidState,
    /// 停止済みセッションへの操作だが、エージェントがネイティブ復元
    /// (`session/resume` 等) に対応していない (履歴 Replay での明示的な再開が必要)
    ResumeRequired,
    PtyDisabled,
    CommandDuplicate,
    NotFound,
    Internal,
}
```

---

## 2. Node ⇔ Central Server 間プロトコル (WebSocket & Stdio Pipe 共通)

常駐ノードと一時VMノードは、トランスポート層が異なるだけで**100%同一のメッセージ型
(`NodeToServerMsg` / `ServerToNodeMsg`)** を共有します：

- **トランスポート A（常駐ノード用: Outbound WebSocket）**:
  - **エンドポイント**: `wss://<server-host>/api/v1/node/ws`
  - **認証**: `Authorization: Bearer <NODE_TOKEN>` ヘッダ（ノード個別トークン。
    `server.db.nodes.token_hash` と照合し、`NodeHello.node_id`
    がトークン発行対象ノードと 一致することを検証してなりすましを拒否する）
- **トランスポート B（一時VM・サンドボックスノード用: Stdio Pipe
  `fxg daemon --stdio`）**:
  - **チャネル**:
    中央サーバーが子プロセスとして起動したプロビジョナーコマンド（`docker`,
    `incus`, `uvx google-colab-cli`, `ssh` 等）の **`stdout`
    (`NodeToServerMsg`)** および **`stdin` (`ServerToNodeMsg`)**
  - **フレーミング**: JSON
    Lines（1メッセージ1行の改行区切りJSON）。ブートストラップや `mise` / `uv`
    によるツール導入ログはすべて **`stderr`** に出力され、`BootstrapLog`
    イベントとしてUIへストリーム配信されます。

```rust
/// ノードが保持するセッションの同期状態（NodeHello で報告）
/// ハブは自 DB の last_node_seq と比較し、欠落・遅延があれば ResyncRequest を返す
pub struct SessionSyncState {
    pub session_id: String,
    pub last_node_seq: u64,
}

/// ハブ側で欠落・遅延しているセッションの再送指定（ResyncRequest）
pub struct ResyncTarget {
    pub session_id: String,
    pub from_node_seq: u64, // この値以降 (>=) のイベントを水位に関係なく再送する
}

/// Node -> Server への送信メッセージ
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NodeToServerMsg {
    /// 接続直後のハンドシェイク（ノード情報・プロジェクト紐付け・セッション同期状態一覧を通知）
    NodeHello {
        node_id: String,
        name: String,
        os: String,
        arch: String,
        version: String,
        is_ephemeral: bool,
        installed_agents: Vec<String>,
        projects: Vec<NodeProjectReport>,
        sessions: Vec<SessionSyncState>,
    },
    /// 永続化イベントのバッチ送信 (Store-and-Forward Outbox)
    /// ※セッションのメタデータ (SessionCreated 等) も session_events 上のイベントとして送信される。
    ///   通常は synced_up_to_node_seq より後のイベントを node_seq 昇順で送る
    EventBatchPush {
        events: Vec<SessionEventEnvelope>,
    },
    /// リアルタイム・エフェメラルチャンク (DBフラッシュ前の高速表示用)
    LiveStreamDelta {
        session_id: String,
        delta: StreamDeltaPayload,
    },
    /// サーバーからのコマンド実行結果応答 (要求元クライアントへ command_id 付きで返却される)
    CommandResult {
        command_id: String,
        success: bool,
        code: Option<ErrorCode>,   // 失敗時の構造化エラーコード
        error: Option<String>,     // 人間向けメッセージ
        session_id: Option<String>,
    },
    /// PTY 出力データチャンク (Web Terminal -> クライアント)
    PtyOutput {
        pty_id: String,
        data_b64: String,
    },
    /// PTY プロセス終了通知
    PtyExit {
        pty_id: String,
        exit_code: Option<i32>,
    },
    /// ノードのGit Diff取得結果応答
    GitDiffResult {
        request_id: String,
        diff: Option<WorkspaceDiffResponse>,
        error: Option<String>,
    },
    /// Worktree 操作 (`ManageWorktree`) の結果応答
    /// ※成功時の Worktree 情報 (削除時は None) を返す
    ///   (WorktreeInfo は §3.1 の client_api 型)
    WorktreeResult {
        command_id: String,
        worktree: Option<WorktreeInfo>,
        error: Option<String>,
    },
    /// セッション Revert (`RevertSession`) の結果応答
    /// ※成功時は復元結果 (SessionRevertResponse は §3.1 の client_api 型) を返す
    RevertResult {
        command_id: String,
        success: bool,
        code: Option<ErrorCode>,   // 失敗時の構造化エラーコード (BUSY 等)
        outcome: Option<SessionRevertResponse>,
        error: Option<String>,
    },
    /// セッション再開 (`ResumeSession`) の結果応答
    ResumeResult {
        command_id: String,
        success: bool,
        code: Option<ErrorCode>,   // 失敗時の構造化エラーコード (INVALID_STATE 等)
        context_restored: Option<bool>, // ネイティブ復元成否 (false = 履歴 Replay で継続)
        error: Option<String>,
    },
    /// セッションのアーカイブ/復元 (`ArchiveSession`) の結果応答
    ArchiveResult {
        command_id: String,
        success: bool,
        code: Option<ErrorCode>,
        archived_at: Option<i64>,  // アーカイブ日時 (Unix epoch ms)。復元時は None
        error: Option<String>,
    },
    /// セッション削除 (`DeleteSession`) の結果応答
    DeleteResult {
        command_id: String,
        success: bool,
        code: Option<ErrorCode>,
        error: Option<String>,
    },
    /// プロジェクト一括スキャン (`ProjectScan`) の結果応答
    ProjectScanResult {
        request_id: String,
        response: Option<ProjectScanResponse>, // scanned_dirs + projects (§3.1)
        error: Option<String>,
    },
    /// プロジェクト手動紐付け (`ProjectLink`) の結果応答
    ProjectLinkResult {
        request_id: String,
        response: Option<ProjectLinkResponse>, // project_id + local_path (§3.1)
        error: Option<String>,
    },
    /// ACP Registry カタログ (`ListAgents`) の結果応答
    AgentsResult {
        request_id: String,
        response: Option<AgentsResponse>, // registry + installed_versions (§3.1)
        error: Option<String>,
    },
    /// エージェント管理操作 (`ManageAgent`) の結果応答
    AgentOpResult {
        request_id: String,
        success: bool,
        code: Option<ErrorCode>,
        message: Option<String>,   // 成功時の表示メッセージ
        error: Option<String>,
    },
    /// PTY 起動・操作の失敗通知 (エラーコードは文字列。例: `PTY_DISABLED`)
    /// ※ノード側設定 allow_remote_pty = false の場合、リモートからの
    ///   PtySpawn に対して PTY_DISABLED を返す
    PtyError {
        pty_id: String,
        code: String,
        message: String,
    },
    /// 一時VMからのオンメモリGit認証要求 (GIT_ASKPASS プロキシ)
    GitCredentialRequest {
        request_id: String,
        repo_url: String,
        operation: String, // "clone" | "fetch" | "push"
    },
    /// 一時VM破棄前の作業ツリー・コミット履歴バンドル退避 (`git bundle create`)
    /// ※bundle は圧縮して Base64 化し、1メッセージ上限 (例: 16 MiB) を超える場合は分割転送する
    WorkspaceBundleUpload {
        session_id: String,
        branch: String,
        head_commit: String,
        bundle_b64: String,
    },
    /// Graceful Drain 完了通知 (これを受信後に中央サーバーが子プロセス/VMを破棄)
    DrainComplete {
        node_id: String,
    },
}

/// Server -> Node への送信メッセージ
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ServerToNodeMsg {
    /// EventBatchPush に対する永続化完了ACK
    /// ※バッチはセッション単位に分割して送信し、ACK もセッション単位で返却する
    ///   ノードは受信後 sessions.synced_up_to_node_seq を水位として更新する
    EventBatchAck {
        session_id: String,
        acked_up_to_node_seq: u64,
    },
    /// NodeHello への応答: ハブ側で欠落・遅延しているセッションの再送要求
    /// ノードは指定 from_node_seq 以降のイベントを水位に関係なく再送する
    /// (from_node_seq = 1 は SessionCreated を含む全量再送を意味する)
    ResyncRequest {
        sessions: Vec<ResyncTarget>,
    },
    /// クライアント(Web/Android)からのセッション新規起動要求
    StartSession {
        command_id: String,
        session_id: String,
        project_id: String,
        local_path: String,
        agent_id: String,
        initial_prompt: Option<String>,
        fork_context_messages: Option<Vec<ForkHistoryItem>>, // 別ノードからの履歴引き継ぎ時
        restore_git_bundle_b64: Option<String>,              // 一時VMから退避されたGitバンドルの復元用
    },
    /// プロンプト送信（スラッシュコマンド含む）
    /// ※停止済みセッションへの送信はノード側でネイティブ復元 (自動レジューム) を
    ///   試みる。非対応エージェントでは `RESUME_REQUIRED` を返し、クライアントは
    ///   明示的な再開 (`SessionResume` = 履歴 Replay) を案内する (設計: docs/04 §4.3)
    SendPrompt {
        command_id: String,
        session_id: String,
        text: String,
        client_source: String,
    },
    /// 承認リクエストへの回答
    RespondPermission {
        command_id: String,
        session_id: String,
        request_id: String,
        selected_option_id: String,
        resolved_by: String,
    },
    /// 質問 (elicitation) への回答
    RespondElicitation {
        command_id: String,
        session_id: String,
        elicitation_id: String,
        action: ElicitationAction,    // "accept" | "decline" | "cancel"
        content: serde_json::Value,   // accept 時の form 回答 (それ以外は null)
        resolved_by: String,
    },
    /// モード切替 / 設定変更 / キャンセル
    ControlSession {
        command_id: String,
        session_id: String,
        action: SessionControlAction, // SetMode(String) | SetConfig(k, v) | Cancel | Kill
    },
    /// 指定ターン時点へのファイル復元 (`fxg session revert`)
    /// ※ノードの Shadow Git Tree からワークスペースを復元し、
    ///   SessionReverted イベントを追記する (実行中セッションのみ。busy 時は BUSY)
    RevertSession {
        command_id: String,
        session_id: String,
        target_node_seq: Option<u64>, // Revert 基準の UserMessage.node_seq (None は直近ターン)
    },
    /// 停止済みセッションの再開 (`POST /api/v1/sessions/:id/resume`)
    /// ※同一 session_id のままエージェントを起動し、ネイティブ復元
    ///   (`session/resume` → `session/load`) を
    ///   優先する。復元できない場合は履歴 Replay を注入して継続する (設計: docs/04 §4.3)
    ResumeSession {
        command_id: String,
        session_id: String,
        force_replay: bool, // ネイティブ復元を試みず履歴 Replay で継続する
    },
    /// セッションのアーカイブ/復元 (`POST /api/v1/sessions/:id/archive`)
    /// ※アーカイブは一覧からの非表示/復元のみで、イベントログは保持される
    ArchiveSession {
        command_id: String,
        session_id: String,
        archived: bool,     // true = アーカイブ, false = 復元
    },
    /// セッションの削除 (`DELETE /api/v1/sessions/:id`)
    /// ※稼働中セッションは停止を待ってから `SessionDeleted` (tombstone) を
    ///   追記し、イベント本文をパージする (復元不能)
    DeleteSession {
        command_id: String,
        session_id: String,
    },
    /// ワークスペースWebターミナル (PTY) の起動要求
    PtySpawn {
        pty_id: String,
        session_id: String,
        cols: u16,
        rows: u16,
        shell_cmd: Option<String>,
    },
    /// PTY へのユーザー入力送信 (キー入力)
    PtyInput {
        pty_id: String,
        data_b64: String,
    },
    /// PTY ウィンドウリサイズ
    PtyResize {
        pty_id: String,
        cols: u16,
        rows: u16,
    },
    /// PTY プロセス終了
    PtyKill {
        pty_id: String,
    },
    /// ノードのGit作業ツリー/ブランチDiff取得要求
    GetGitDiff {
        request_id: String,
        session_id: String,
        scope: DiffScope,              // Uncommitted (git diff HEAD) | BranchBase (git diff <base>...HEAD)
        base_branch: Option<String>,
    },
    /// Worktree 操作要求 (作成・削除・一覧)
    ManageWorktree {
        command_id: String,
        project_id: String,
        action: WorktreeAction,        // Add { branch, new_path } | Remove { path } | Prune
    },
    /// 指定ディレクトリ配下の Git リポジトリ一括スキャン・登録要求 (`fxg project scan`)
    ProjectScan {
        request_id: String,
        dir: Option<String>,          // 未指定時は node.project_scan_dirs
    },
    /// 任意ディレクトリの論理プロジェクトへの手動紐付け要求 (`fxg project link`)
    ProjectLink {
        request_id: String,
        project_id: String,
        local_path: String,
    },
    /// ACP Registry カタログ + 導入状態の取得要求 (Web UI のエージェント管理)
    ListAgents {
        request_id: String,
    },
    /// エージェント管理操作要求 (install / update / remove)
    ManageAgent {
        request_id: String,
        action: AgentAction,          // Install { agent_id } | Update { agent_id? } | Remove { agent_id }
    },
    /// GitCredentialRequest に対する短命トークン応答
    GitCredentialResponse {
        request_id: String,
        username: String,
        token: Option<String>,
        error: Option<String>,
    },
    /// 一時VMの終了前フラッシュ＆Gitバンドル退避要求
    DrainAndShutdown {
        reason: String,
        create_git_bundle: bool,
    },
    /// 緊急キルスイッチ: ノード上で稼働中の全セッション・プロセスツリー・PTYを即時強制停止
    KillAllSessions {
        reason: String,
    },
}
```

---

## 3. Client (PWA / Browser) ⇔ Server (または Local Node) API

中央サーバー (`fxg server`) と各ノードのローカルWebサーバー (`fxg daemon` on
`localhost:7860`)
は**完全に同一のAPIパスとWebSocketフォーマット**を実装します。これにより、PWAフロントエンドは接続先URLを意識せずにどちらにも繋がります。

### 3.0 共通セキュリティ & 認証仕様

すべてのHTTPおよびWebSocketリクエストは以下のセキュリティ検証を通過する必要があります：

1. **認証方式**:
   - `Authorization: Bearer <auth_token>`
     ヘッダ、または初回トークン検証時 (`POST /api/v1/auth/login`) に発行される
     `Cookie: fxg_session=<token>; HttpOnly; SameSite=Strict`。
   - 未認証リクエストは即座に `401 Unauthorized` を返却。例外は認証不要の
     メタ情報 `GET /api/v1/meta` のみ (Host / Origin 検証は適用)。
2. **Host ヘッダ検証 (DNS Rebinding 防御)**:
   - リクエストの `Host` ヘッダが `localhost:<port>`, `127.0.0.1:<port>`,
     またはサーバー設定の許可ホスト（例: Tailscale MagicDNS名 /
     LANホスト名）に一致しない場合、`403 Forbidden` を返却。
3. **Origin ヘッダ検証 (Cross-Site WebSocket Hijacking 防御)**:
   - WebSocketハンドシェイク時、`Origin`
     が自サーバーのドメインまたはローカルオリジン以外からの接続である場合、ハンドシェイクを拒否。
4. **監査ログ記録 (Audit Logging)**:
   - `session` 起動、`permission` 解決、`kill-switch` 実行、`worktree`
     操作、セッション Revert、プロジェクト紐付け (scan / link)、エージェント管理
     (install / update / remove)、ノードトークン発行・失効、クライアントトークン
     再生成は、クライアントIP・UA・トークンIDとともに `audit_logs` に記録。
   - 記録先は中央サーバー経由の操作が
     `server.db`、ローカル直結（`localhost:7860` / CLI）の操作が 実行ノードの
     `node.db`（どちらも `/api/v1/audit/logs` で参照可能）。
5. **エラーレスポンスの共通形式**:
   - REST / WS のエラーは
     `{ "error": { "code": "<ErrorCode>", "message": "..." } }` に統一 （例:
     `NODE_OFFLINE`, `ALREADY_RESOLVED`, `PTY_DISABLED`）。

### 3.1 REST API エンドポイント

- `GET /api/v1/meta`: 認証不要の接続先メタ情報。`{ "role": "central_server" |
  "local_node" }` を返却する。Web UI
  は接続先種別の判定をこのエンドポイントのみで行い（認証済み
  `system/info` の `role` やホスト名からは推測しない）、トークン入力ダイアログ
  の表示（中央サーバー /
  ローカルノード）と確認方法の案内を切り替える。取得失敗は接続エラーとして扱う
  (Host / Origin 検証は適用)。
- `GET /api/v1/system/info`: 接続先が `central_server` か `local_node`
  か、および Web Push の VAPID Public Key を返却。ローカルノード接続時は
  `unsynced_event_count`（Outbox 残数）と
  `central_connected`（中央サーバー接続状態）も返却。
- `POST /api/v1/auth/login`: Web UI のトークン入力フロー。認証済みリクエスト
  (`Authorization: Bearer <auth_token>`) と `{ "token": "..." }` を受け取り、
  body のトークンを再検証したうえで
  `Set-Cookie: fxg_session=<token>; HttpOnly; SameSite=Strict; Path=/` を返す
  (204)。以降は Cookie 認証 (Service Worker の Push 承認応答等) が使える。
- `POST /api/v1/auth/logout`: `fxg_session` Cookie を失効させる
  (`Max-Age=0`、204)。
- `GET /api/v1/projects`: プロジェクト一覧と、各プロジェクトに紐づくノードおよび
  Worktree（`project_node_bindings`）を返却。
- `GET /api/v1/projects/:id/worktrees`: 指定プロジェクトの各ノード上にある
  Worktree 一覧（パス、ブランチ、HEADコミット）を取得。
- `POST /api/v1/projects/:id/worktrees`: 指定ノード上で新規
  Worktree（`git worktree add -b <branch> <path>`）を作成。
- `DELETE /api/v1/projects/:id/worktrees`: 指定ノード上の Worktree
  を削除（`git worktree remove`）。
- `POST /api/v1/projects/:id/worktrees/prune`:
  指定ノード上のメインリポジトリで `git worktree prune`
  を実行し、削除済みディレクトリの管理情報をクリーンアップする
  (`fxg worktree prune` の Web UI 版。`{ node_id }`)。
- `POST /api/v1/nodes/:node_id/projects/scan`:
  指定ディレクトリ (省略時は `config.toml` の `node.project_scan_dirs`)
  配下の Git リポジトリを一括探索し、論理プロジェクトとして登録する
  (`fxg project scan` の Web UI 版)。`{ dir: Option<String> }` を受け取り、
  `ProjectScanResponse` (`scanned_dirs` + ノード上での登録プロジェクト一覧)
  を返す。
- `POST /api/v1/nodes/:node_id/projects/link`:
  任意ディレクトリを論理プロジェクト ID へ手動で紐付ける
  (`fxg project link` の Web UI 版。対象の `.fxg.toml` に `project_key`
  を書き込む)。`{ project_id, local_path }` を受け取り、解決結果
  (`ProjectLinkResponse`) を返す。
- `GET /api/v1/nodes`:
  ノード一覧とオンライン状態、一時ノード属性（`is_ephemeral`,
  `lifecycle_status`）、利用可能エージェント一覧を返却。
- `GET /api/v1/provisioners`:
  利用可能な一時VM・サンドボックスプロビジョナー一覧（`local-docker`,
  `local-incus`, `colab-pro` 等）を返却。
- `POST /api/v1/provisioners/:name/test`:
  指定プロビジョナーの起動と `fxg daemon --stdio` の `NodeHello`
  ハンドシェイク疎通を検証する（中央サーバーのみ。プロビジョナーはサーバーホスト上で
  子プロセス起動されるため）。`ProvisionerTestResponse`
  (`ok` / `node_id` / `stderr` ログ末尾 / `error`)
  を返却し、検証後は一時環境を破棄する。
- `GET /api/v1/sessions?project_id=...&status=...&include_archived=true`:
  セッション一覧。削除済み (tombstone) は常に除外され、アーカイブ済みは
  `include_archived=true` 指定時のみ返却される (既定は除外)。
- `POST /api/v1/sessions`: 新規セッションの開始（既存の常駐 `node_id`
  指定のほか、`provisioner`
  指定による一時VMのオンデマンド起動＆自動セットアップ、Worktreeパス指定、別ノードや退避済みGitバンドルからのContext
  Forkに対応）。
- `GET /api/v1/sessions/:id/events?after_cursor=0`:
  指定カーソル以降のイベント履歴取得（`SessionEventBatch` を返却）。
- `GET /api/v1/sessions/:id/diff?scope=uncommitted&base=main`:
  ノードのリアルタイムGit差分を取得。
  - `scope=uncommitted` (デフォルト): 現在の作業ツリー未コミット差分
    (`git diff HEAD`)
  - `scope=branch`: ベースブランチとの累積差分 (`git diff <base>...HEAD`)
- `GET /api/v1/inbox`: 全セッション横断の未解決 `PermissionRequest` と
  `ElicitationRequest` の一覧 (`{ requests, elicitations }`)。
- `POST /api/v1/sessions/:id/permissions/:req_id/respond`:
  承認リクエストへの応答 (`selected_option_id`, `always`,
  `resolved_by`)。既に解決済みの場合は `ALREADY_RESOLVED` を返却し（冪等）、UI
  側は正常遷移として扱う。
- `POST /api/v1/sessions/:id/elicitations/:elicitation_id/respond`:
  質問 (elicitation) への応答 (`action` = `accept` | `decline` | `cancel`,
  `content` = `accept` 時の form 回答, `resolved_by`)。既に解決済みの場合は
  `already_resolved = true` を返却し（冪等）、UI 側は正常遷移として扱う。
- `POST /api/v1/sessions/:id/revert`:
  指定ターン (`target_node_seq` = `UserMessage.node_seq`) 時点の Shadow Git Tree
  へワークスペースのファイルを復元する (`fxg session revert` の Web UI 版)。
  会話イベントは削除せず `SessionReverted` を追記し、復元直前の状態は
  バックアップ Tree Hash として退避する。応答は `SessionRevertResponse`
  (`target_node_seq` / Tree Hash / 復元・削除ファイル数)。実行中セッションのみ
  対象で、ターン実行中は `BUSY` を返却する。中央サーバー経由の場合は対象ノードへ
  `RevertSession` を中継する。
- `POST /api/v1/sessions/:id/resume`:
  停止済みセッションを同一 `session_id` のまま再開する (`fxg session resume`
  の Web UI 版)。エージェント側コンテキストのネイティブ復元
  (`session/resume` → `session/load`) を優先し、
  非対応の場合は履歴 Replay を注入して継続する。応答は `ResumeSessionResponse`
  (`session_id` / `context_restored`)。稼働中セッションは `INVALID_STATE`、
  一時VMセッションは v1 では `INVALID_STATE`
  を返却する。中央サーバー経由の場合は
  対象ノードへ `ResumeSession` を中継する (設計: docs/04 §4.3)。
- `POST /api/v1/sessions/:id/archive`:
  セッションをアーカイブ/復元する (`{ "archived": true | false }`。
  `fxg session archive` / `unarchive` の Web UI 版)。アーカイブは
  一覧からの非表示/復元のみで、イベントログは保持される。応答は
  `SessionArchiveResponse` (`session_id` / `archived_at`)。中央サーバー
  経由の場合は対象ノードへ `ArchiveSession` を中継する。
- `DELETE /api/v1/sessions/:id`:
  セッションを削除する (`fxg session delete` の Web UI 版。204 No Content)。
  稼働中セッションは停止を待ってから `SessionDeleted` (tombstone) を追記し、
  本文イベント・承認履歴をパージする (**復元不能**)。
  アーカイブ済みでも削除できる。中央サーバー経由の場合は対象ノードへ
  `DeleteSession` を中継する。
- `POST /api/v1/search?q=...`: SQLite FTS5 を用いた全セッション横断の全文検索。
- `POST /api/v1/push/subscribe`: Android / Desktop PWA の Web Push
  サブスクリプション登録 (中央サーバーのみ)。VAPID 鍵は初回起動時に
  `~/.flexagent/vapid.json` へ自動生成され、公開鍵は `GET /api/v1/system/info`
  の `vapid_public_key` (base64url) で配布される。
- `POST /api/v1/system/kill-switch`: **緊急停止 (Panic
  Button)**。全ノードの稼働中セッション、実行中プロセスツリー、PTYを一括強制終了。
- `GET /api/v1/audit/logs?limit=50`:
  監査ログ（操作日時、操作種別、送信元IP、クライアント種別）の取得。中央サーバーでは
  `server.db`、 ローカルノードでは `node.db` の `audit_logs` を参照する。
- `GET /api/v1/agents`:
  ACP Registry
  カタログと導入状態を返す。中央サーバーではオンラインのいずれかのノード
  (`ListAgents` 中継) からカタログを取得し、全ノードの
  `NodeHello.installed_agents`
  を統合して `installed_nodes` (導入済みノード一覧) を埋める。
- `POST /api/v1/nodes/:node_id/agents/:agent_id/install`:
  指定ノード上で ACP Registry からエージェントをダウンロード・展開する
  (`fxg agents install` の Web UI 版)。
- `POST /api/v1/nodes/:node_id/agents/update`:
  指定ノードの導入済みエージェントを更新する (`fxg agents update` の Web UI 版。
  `{ agent_id: Option<String> }`。省略時は導入済み全件)。
- `DELETE /api/v1/nodes/:node_id/agents/:agent_id`:
  指定ノード上のキャッシュ済みバイナリを削除する (`fxg agents remove` の Web UI
  版)。
- `POST /api/v1/auth/rotate-token`:
  クライアント認証トークン (`auth_token`) を再生成し `{ token }` を返す。
  旧トークン (Bearer / `fxg_session` Cookie) は即時無効化される。
- `GET /api/v1/nodes/tokens`:
  発行済みノード個別トークン一覧 (`node_id` + トークンハッシュ先頭 12 文字。
  平文は保存しない)。中央サーバーのみ。
- `POST /api/v1/nodes/tokens`:
  ノード個別トークンを発行
  (`{ node_id }`)。平文トークンは**発行時の応答でのみ**返却し、
  `server.db.nodes.token_hash` には SHA-256
  ハッシュを保存する。中央サーバーのみ。
  未登録のノードはプレースホルダで登録し `NodeHello` で上書きされる。
- `DELETE /api/v1/nodes/tokens/:node_id`:
  ノード個別トークンを失効させる。中央サーバーのみ。

### 3.2 Client WebSocket (`/api/v1/client/ws`)

1. 接続時にクライアントが
   `Subscribe { since_cursor: Option<u64>, focused_session_id: Option<String> }`
   を送信する。
   - `since_cursor` は**接続先ストア**（中央サーバーなら
     `server.db`、ローカルノードなら `node.db`）の `session_events.cursor`
     を指定する。接続先を切り替えた場合は
     もう一方のカーソルは使えないため、`None`（初回全量）から開始する。
2. サーバー / ノードは該当カーソル以降の未取得イベントを
   `SessionEventBatch { events, cursor }`
   として即座に流し、以降はリアルタイムイベント（同バッチ形式および
   `LiveStreamDelta`）をプッシュします。
   - クライアントは最後に受信したバッチの `cursor` を保存し、再接続時に
     `since_cursor` へ渡す。
   - `since_cursor` が**ストアの末尾（最大カーソル）より大きい場合は無効**
     と判定し、0 から全量リプレイする（DB 再作成・リセット後の古いカーソルで
     新ストアのイベントを全てスキップしてしまう事故の防御）。
   - クライアントはバッチのカーソル巻き戻り（受信カーソルが保存値より小さい）
     を検知したら、保存済みタイムラインを破棄して REST 投影を再取得し、
     新しいストアとして再同期する。
3. クライアントからの操作（`SendPrompt`, `RespondPermission`,
   `RespondElicitation`, `ControlSession`）もこのWebSocket上（またはREST
   POST）で送信でき、結果は `command_id` 付きの `CommandResult`
   として要求元クライアントへ応答されます。
   - **一時VMブートストラップログ (`BootstrapLog`)**:
     一時VMの起動中（`fxg daemon
     --stdio` の `NodeHello` 前）はノードの `node.db` が存在しないため、中央
     サーバーがプロビジョナーの `stderr` を `{ op: "bootstrap_log", session_id,
     line }` として**エフェメラル配信**する（イベントログには永続化されず、
     再接続時は復元されない）。UI はセッション詳細の「Environment Bootstrap
     Log」カードにリアルタイム表示する。
4. **重複排除とマージ**:
   - クライアントは受信イベントを `event_id` / `(session_id, node_seq)` をキーに
     upsert し、重複配信 （`LiveStreamDelta`
     と永続イベント、再接続時のリプレイ）を無害化する。
   - ターン途中の `LiveStreamDelta` は `message_id` / `thought_id`
     でマージ表示し、永続イベント
     （`is_complete = true`）到着時に確定表示へ置き換える。
   - セッション内の表示順は `node_seq` を正とし、`cursor`
     は再開位置の記録にのみ使用する。

### 3.3 Client 双方向 Web PTY WebSocket (`/api/v1/pty/ws`)

Web UI / スマホPWA上のターミナル（`ghostty-web` /
xterm互換アダプター）とノード上の ConPTY / Unix PTY
を直接結ぶ超低遅延バイナリ/JSONストリームチャネルです。

- **セキュリティ制御 (`allow_remote_pty`)**:
  - ノード側設定で `allow_remote_pty = false`
    の場合、リモート（中央サーバー経由）からの `spawn` 要求に対して
    `{ op: "error", code: "FORBIDDEN", message: "Remote PTY is disabled on this node by security policy" }`
    を返し、接続を切断します。
- クライアントから接続時に `{ op: "attach", pty_id: "..." }` または
  `{ op: "spawn", session_id: "...", cols: 80, rows: 24 }` を送信。
- ユーザーのキー入力は `{ op: "input", data_b64: "..." }`
  で即座にノードのPTY標準入力へ書き込まれます。
- 画面リサイズ時は `{ op: "resize", cols: N, rows: M }` を送信し、ConPTY / Unix
  PTYのウィンドウサイズを動的変更します。

---

## 4. CLI (`fxg`) ⇔ Local Daemon (`fxg daemon`) 間 ローカルIPC

- **トランスポート**:
  - **Windows**: Named Pipe (`\\.\pipe\fxg-daemon-<username>`)
  - **Linux / macOS / WSL**: Unix Domain Socket
    (`$XDG_RUNTIME_DIR/fxg/daemon.sock` または `/tmp/fxg-<uid>/daemon.sock`)
- **フレーミング**: Length-prefixed JSON（4バイトのリトルエンディアン長さヘッダ
  ＋ JSONペイロード）

### 主なIPCメソッド

1. `EnsureSession { cwd, agent_id, extra_args, ... } -> { session_id, title }`:
   - `fxg run <agent>` 実行時に呼ばれ、デーモン側でセッションを開始します。
2. `GetLocalStatus`:
   - ローカルで稼働中のセッション一覧、中央サーバーとのWebSocket接続状態、未送信Outboxイベント数を返却。
3. `SessionResume { session_id, force_replay } -> { session_id, context_restored }`:
   - 停止済みセッションを同一 `session_id` のまま再開します
     (`fxg session resume`)。ネイティブ復元を優先し、非対応時は履歴 Replay で
     継続します (`context_restored = false`)。稼働中の再開は `INVALID_STATE`
     を返却します。
   - `SendPrompt` は停止済みセッションに対してこの再開 (ネイティブ限定) を
     自動実行してから送信します。ネイティブ復元不可の場合は `RESUME_REQUIRED`
     を返すため、クライアントは `SessionResume` での履歴 Replay
     再開を案内します。
4. `SessionArchive { session_id, archived } -> { session_id, archived_at }`:
   - セッションのアーカイブ/復元 (`fxg session archive` / `unarchive`)。
     アーカイブは一覧からの非表示/復元のみで、イベントログは保持されます。
5. `SessionDelete { session_id } -> { session_id }`:
   - セッションの削除 (`fxg session delete`)。稼働中は停止を待ってから
     `SessionDeleted` (tombstone) を追記し、本文イベントをパージします
     (復元不能)。
6. `ListSessions { include_stopped, include_archived }`:
   - セッション一覧 (`fxg ps` / `fxg session list`)。削除済みは常に除外され、
     アーカイブ済みは `include_archived` 指定時のみ含まれます。
