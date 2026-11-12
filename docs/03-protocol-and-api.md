# 03. プロトコル・API仕様 (`fxg-protocol`)

`crates/fxg-protocol` クレートに定義する共通データ型と、2つの通信経路（① Node ⇔
Hub WebSocket/Stdio、② Client ⇔ Hub/Node HTTP+WS）のプロトコル仕様です。
CLI (`fxg`) はローカルノードの HTTP REST API / WebSocket（`node.addr`、既定は
`127.0.0.1:7860`）またはハブの API を直接利用し、プラットフォーム固有の
ローカルIPC (Named Pipe / UDS) は全廃されました。
クライアント (UI / CLI) が直接扱う型には
`#[derive(Serialize, Deserialize, specta::Type)]` + `#[specta(collect)]`
を付与し、`ui/src/lib/generated/bindings.ts`（SvelteKit の
`$lib/generated/bindings`）へTypeScriptの型定義を自動出力します（設定ファイル
スキーマ等のハブ内部型は `specta` export 対象外とする）。

---

## 1. 正規化セッションイベント型 (`UnifiedEventPayload`)

ACP (`agent-client-protocol-schema`)
のイベントモデルをベースに、ターミナル出力や承認解決イベントを統合した型です。

```rust
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionEventEnvelope {
    pub event_id: String,       // UUID v7 (冪等適用・重複排除)
    pub session_id: String,     // UUID v7
    pub node_seq: u64,          // セッション内連番 (実行ノードのみが採番。順序の正)
    pub created_at: i64,        // Unix epoch ms
    pub payload: UnifiedEventPayload,
}

/// カーソルを伴うイベントバッチ (Client WS 配信 / 履歴 API の共通型)
/// cursor は「配信元ストア (node.db / hub.db) への取り込み順」。
/// クライアントは最後に受信したバッチの cursor を保存し、再接続時に
/// Subscribe { since_cursor } へ渡す (接続先ストア以外の cursor は意味を持たない)
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionEventBatch {
    pub events: Vec<SessionEventEnvelope>,
    pub cursor: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[specta(collect)]
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
        title: String,
    },
    /// セッションタイトル変更 (UI/CLI からのリネーム。コマンドとして実行ノードに到達してから発行される)
    SessionTitleChanged {
        title: String,
    },
    /// セッション単位の承認ポリシー変更 (`ask` / `auto_allow_all`)
    SessionPermissionPolicyChanged {
        policy: PermissionPolicy,
    },
    /// ACP セッション確立などによる agent_session_id の確定
    SessionAgentBound {
        agent_session_id: String,
    },
    /// ユーザーが送信したプロンプト（スラッシュコマンド含む）
    UserMessage {
        text: String,
        attachments: Vec<AttachmentMeta>,
        client_source: String,        // "cli" | "web" | "android" | "resume"
    },
    /// プロンプト入力（Client WS / Node⇔Hub / REST 共通）
    PromptInput {
        text: String,
        attachments: Vec<AttachmentMeta>,
    },
    /// 添付メタデータ。画像は `data_b64` (インライン) または `local_path`
    /// (ノード上のファイル)、その他は `local_path` を使う。
    AttachmentMeta {
        file_name: String,
        mime_type: String,
        size_bytes: u64,
        local_path: Option<String>,
        data_b64: Option<String>,
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
    /// 一時VMプロビジョニング (stderr) の進捗ログ行 (永続化対象)
    ProvisioningLog {
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
    /// UI / CLI はタイムラインにシステム行として表示し、文言は reason から導出する
    TurnEnded {
        reason: TurnStopReason,  // "max_tokens" | "max_turn_requests" | "refusal"
    },
    /// コンテキスト圧縮 (compaction) の進行状況 (永続化対象)
    CompactionUpdated {
        status: CompactionStatus, // "started" | "completed" | "failed"
        detail: Option<String>,   // 失敗理由などの補足 (任意)
    },
    /// セッション状態の変化 ('provisioning' | 'idle' | 'running' | ...)
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
すべてクライアントが扱うため `#[derive(Serialize, Deserialize, specta::Type)]` +
`#[specta(collect)]` を付与します。

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
    pub title: String,
    pub status: SessionStatus,
    pub current_mode: Option<String>,
    /// 最新のコンテキスト使用量・累積コスト (UsageUpdated の投影。None = 未受信)
    pub usage: Option<SessionUsage>,
    pub last_node_seq: u64,
    /// エージェントがACPネイティブFork (`session/fork` + `session/load`) に対応しているか
    pub fork_supported: bool,
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
    pub description: Option<String>,
    /// UX ヒント (model / thought_level / mode など)。未知値は保持する。
    pub category: Option<ConfigOptionCategory>,
    pub current_value: serde_json::Value,
    pub options: Vec<serde_json::Value>,
}

#[serde(rename_all = "snake_case")]
pub enum SessionStatus { Provisioning, Idle, Running, WaitingPermission, WaitingInput, Stopped, Error }

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

## 2. Node ⇔ Hub 間プロトコル (WebSocket & Stdio Pipe 共通)

`NodeHello` は `state_revision` (単調増加) と `projects_complete` を含む全量
スナップショットです。ハブは古い `state_revision` を無視し、
`projects_complete = true` のときだけ今回のレポートに含まれない
`project_node_bindings` を prune します。`SessionSyncState` は各セッションの
現在の `project_id` を含み、ハブはバインディング / レポートに基づいて
`sessions.project_id` を更新します。

### 2.1 バージョン契約

`NodeHello.protocol_version` に
`fxg_protocol::node_hub::PROTOCOL_VERSION` を載せます。ハブは最小
バージョン未満のノードを `NodeHello` の時点で拒否し、必要なバージョンを
ログに残して接続を閉じます。ノードとハブは**同一 `fxg` リリースで
同時に更新する**前提とし、後方互換の分岐は提供しません。旧ノードは
フィールドを送らないため `0` としてデコードされ、拒否されます。

### 2.2 メッセージの分類

- **同期・ストリーム**: `EventBatchPush` / `EventBatchAck`, `ResyncRequest`,
  `LiveStreamDelta`, `PtyOutput` / `PtyExit` / `PtyError`。
  `ResyncRequest` に応じた再送は `EventBatchPush.resync = true` とし、ハブは
  そのバッチの Web Push 通知を抑止します（履歴の再送で端末を起こさない）。
  `NodeHello.sessions_truncated` は報告上限に達したことを示します。
- **セッションコマンド**: `StartSession`, `SendPrompt`, `RespondPermission`,
  `RespondElicitation`, `ControlSession`。結果は `CommandResult` で返します。
- **汎用 Node RPC**: 上記以外のノード操作は `HubToNodeMsg::NodeRpc` と
  `NodeToHubMsg::NodeRpcResult` の1組に集約します。
- **ライフサイクル**: `DrainAndShutdown`, `DrainComplete`, `KillAllSessions`

常駐ノードと一時VMノードは、トランスポート層が異なるだけで**100%同一のメッセージ型
(`NodeToHubMsg` / `HubToNodeMsg`)** を共有します：

- **トランスポート A（常駐ノード用: Outbound WebSocket）**:
  - **エンドポイント**: `wss://<hub-host>/api/v1/node/ws`
  - **認証**: `Authorization: Bearer <NODE_TOKEN>` ヘッダ（ノード個別トークン。
    `hub.db.nodes.token_hash` と照合し、`NodeHello.node_id`
    がトークン発行対象ノードと一致することを検証してなりすましを拒否する）
- **トランスポート B（一時VM・サンドボックスノード用: Stdio Pipe
  `fxg internal stdio`）**:
  - **チャネル**:
    ハブが子プロセスとして起動したプロビジョナーコマンド（`docker`,
    `incus`, `uvx google-colab-cli`, `ssh` 等）の **`stdout`
    (`NodeToHubMsg`)** および **`stdin` (`HubToNodeMsg`)**
  - **フレーミング**: JSON
    Lines（1メッセージ1行の改行区切りJSON）。初期化ログや診断メッセージはすべて
    **`stderr`** に出力され、`ProvisioningLog` イベントとしてUIへストリーム配信されます。

### 2.3 汎用 Node RPC

ハブは操作ごとの個別メソッドを持たず、`NodeRpcRequest` を
`rpc(node_id, request_id, request)` で送り、`NodeRpcResult` を待ちます。
タイムアウトは `NodeRpcRequest::timeout()` に一元化しています。

| operation                                                        | 内容                             | timeout |
| :--------------------------------------------------------------- | :------------------------------- | ------: |
| `GetGitDiff`                                                     | セッションの作業ツリー diff      |   30 秒 |
| `ManageWorktree`                                                 | Worktree の作成・削除・prune     |   30 秒 |
| `BrowseFs`                                                       | ノードのファイルシステム閲覧     |   30 秒 |
| `ProjectScan`                                                    | ディレクトリ配下の一括スキャン   |  120 秒 |
| `ProjectLink`                                                    | 任意ディレクトリの手動紐付け     |   30 秒 |
| `ListAgents`                                                     | ACP Registry カタログ + 導入状態 |  120 秒 |
| `ProbeAgentConfig`                                               | 一時 ACP セッションでモデル一覧  |  120 秒 |
| `GetNodeSettings` / `UpdateNodeSettings` / `UpdateAgentSettings` | 設定の参照・更新                 |   30 秒 |
| `GetSecrets` / `UpsertSecret` / `DeleteSecret`                   | 秘密のメタデータ参照・更新       |   30 秒 |
| `ManageAgent`                                                    | エージェントの導入・更新・削除   |  600 秒 |
| `ResumeSession`                                                  | 停止済みセッションの再開         |  120 秒 |
| `ArchiveSession`                                                 | セッションのアーカイブ / 復元    |   30 秒 |
| `DeleteSession`                                                  | セッションの削除                 |  120 秒 |

`NodeRpcOutcome` は成功時 `Ok { response }`、失敗時 `Err { error: ErrorInfo }`
です。ノード側の `ErrorCode` / `RecoveryAction` / `DataSafety` はハブで
文字列に潰さず、HTTP 境界の `ApiError::from_info` で status に変換するだけに
しています。

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeRpcRequest {
    GetGitDiff {
        session_id: SessionId,
        scope: DiffScope,
        base_branch: Option<String>,
    },
    ManageWorktree {
        project_id: ProjectId,
        action: WorktreeAction,
    },
    BrowseFs {
        path: Option<String>,
    },
    ProjectScan {
        dir: Option<String>,
    },
    ProjectLink {
        project_id: ProjectId,
        local_path: String,
    },
    ListAgents,
    ProbeAgentConfig {
        agent_id: String,
        local_path: String,
    },
    GetNodeSettings,
    UpdateNodeSettings {
        request: UpdateNodeSettingsRequest,
    },
    UpdateAgentSettings {
        request: UpdateAgentSettingsRequest,
    },
    GetSecrets,
    UpsertSecret {
        id: SecretId,
        request: UpsertSecretRequest,
    },
    DeleteSecret {
        id: SecretId,
    },
    ManageAgent {
        action: AgentAction,
    },
    ResumeSession {
        session_id: SessionId,
        force_replay: bool,
    },
    ArchiveSession {
        session_id: SessionId,
        archived: bool,
    },
    DeleteSession {
        session_id: SessionId,
    },
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeRpcResponse {
    GitDiff { diff: WorkspaceDiffResponse },
    Worktree { worktree: Option<WorktreeInfo> },
    BrowseFs { response: FsBrowseResponse },
    ProjectScan { response: ProjectScanResponse },
    ProjectLink { response: ProjectLinkResponse },
    Agents { response: AgentsResponse },
    NodeSettings { snapshot: Box<NodeSettingsSnapshot> },
    Secrets { secrets: Vec<SecretSummary> },
    AgentOp { message: Option<String> },
    Resume { context_restored: bool },
    Archive { archived_at: Option<i64> },
    Delete,
}

#[serde(tag = "status", rename_all = "snake_case")]
pub enum NodeRpcOutcome {
    Ok { response: NodeRpcResponse },
    Err { error: ErrorInfo },
}
```

### 2.4 メッセージ定義 (抜粋)

```rust
/// ノードが保持するセッションの同期状態（NodeHello で報告）
pub struct SessionSyncState {
    pub session_id: SessionId,
    pub project_id: ProjectId,
    pub last_node_seq: u64,
}

/// ハブ側で欠落・遅延しているセッションの再送指定（ResyncRequest）
pub struct ResyncTarget {
    pub session_id: SessionId,
    pub from_node_seq: u64, // この値以降 (>=) のイベントを水位に関係なく再送する
}

/// Node -> Hub への送信メッセージ
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NodeToHubMsg {
    NodeHello {
        node_id: NodeId,
        name: String,
        os: String,
        arch: String,
        version: String,
        protocol_version: u32,
        is_ephemeral: bool,
        installed_agents: Vec<String>,
        projects: Vec<NodeProjectReport>,
        projects_complete: bool,
        state_revision: u64,
        sessions: Vec<SessionSyncState>,
        sessions_truncated: bool, // 報告上限 (1000) に達した
    },
    EventBatchPush {
        events: Vec<SessionEventEnvelope>,
        resync: bool, // ResyncRequest 由来 (ハブは Web Push を抑止する)
    },
    LiveStreamDelta {
        session_id: SessionId,
        delta: StreamDeltaPayload,
    },
    CommandResult {
        command_id: CommandId,
        success: bool,
        error: Option<ErrorInfo>,
        session_id: Option<SessionId>,
        warnings: Vec<WarningInfo>,
        data_safety: Option<DataSafety>,
    },
    NodeRpcResult {
        request_id: CommandId,
        outcome: NodeRpcOutcome,
    },
    PtyOutput { pty_id: String, data_b64: String },
    PtyExit { pty_id: String, exit_code: Option<i32> },
    PtyError { pty_id: String, code: String, message: String },
    DrainComplete { node_id: NodeId },
}

/// Hub -> Node への送信メッセージ
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HubToNodeMsg {
    EventBatchAck {
        session_id: SessionId,
        acked_up_to_node_seq: u64,
    },
    ResyncRequest { sessions: Vec<ResyncTarget> },
    StartSession {
        command_id: CommandId,
        session_id: SessionId,
        project_id: ProjectId,
        local_path: String,
        agent_id: String,
        initial_prompt: Option<PromptInput>,
        initial_config: Vec<InitialConfig>,
        mode: Option<String>,
        extra_args: Option<Vec<String>>,
        fork_from_session_id: Option<SessionId>,
    },
    SendPrompt {
        command_id: CommandId,
        session_id: SessionId,
        input: PromptInput,
        client_source: ClientSource,
    },
    RespondPermission {
        command_id: CommandId,
        session_id: SessionId,
        request_id: String,
        selected_option_id: String,
        resolved_by: ResolvedBy,
    },
    RespondElicitation {
        command_id: CommandId,
        session_id: SessionId,
        elicitation_id: String,
        action: ElicitationAction,
        content: Option<ElicitationContent>,
        resolved_by: ResolvedBy,
    },
    ControlSession {
        command_id: CommandId,
        session_id: SessionId,
        action: SessionControlAction,
    },
    NodeRpc {
        request_id: CommandId,
        request: NodeRpcRequest,
    },
    PtySpawn {
        pty_id: String,
        session_id: SessionId,
        cols: u16,
        rows: u16,
        shell_cmd: Option<String>,
    },
    PtyInput { pty_id: String, data_b64: String },
    PtyResize { pty_id: String, cols: u16, rows: u16 },
    PtyKill { pty_id: String },
    DrainAndShutdown { reason: String },
    KillAllSessions { reason: String },
}
```

---

## 3. Client (PWA / Browser) ⇔ Hub (または Local Node) API

ハブ (`fxg hub`) と各ノードのローカルWebサーバー (`fxg node` on
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
     またはハブ設定の許可ホスト（例: Tailscale MagicDNS名 /
     LANホスト名）に一致しない場合、`403 Forbidden` を返却。
3. **Origin ヘッダ検証 (Cross-Site WebSocket Hijacking 防御)**:
   - WebSocketハンドシェイク時、`Origin`
     が自サーバーのドメインまたはローカルオリジン以外からの接続である場合、ハンドシェイクを拒否。
4. **監査ログ記録 (Audit Logging)**:
   - `session` 起動、`permission` 解決、`kill-switch` 実行、`worktree`
     操作、プロジェクト紐付け (scan / link)、エージェント管理
     (install / update / remove)、ノードトークン発行・失効、クライアントトークン
     再生成は、クライアントIP・UA・トークンIDとともに `audit_logs` に記録。
   - 記録先はハブ経由の操作が
     `hub.db`、ローカル直結（`localhost:7860` / CLI）の操作が 実行ノードの
     `node.db`（どちらも `/api/v1/audit/logs` で参照可能）。
5. **エラーレスポンスの共通形式**:
   - REST / WS のエラーは
     `{ "error": { "code": "<ErrorCode>", "message": "..." } }` に統一 （例:
     `NODE_OFFLINE`, `ALREADY_RESOLVED`, `PTY_DISABLED`）。

### 3.1 REST API エンドポイント

- `GET /api/v1/meta`: 認証不要の接続先メタ情報。`{ "role": "hub" |
  "local_node" }` を返却する。Web UI
  は接続先種別の判定をこのエンドポイントのみで行い（認証済み
  `system/info` の `role` やホスト名からは推測しない）、トークン入力ダイアログ
  の表示（ハブ /
  ローカルノード）と確認方法の案内を切り替える。取得失敗は接続エラーとして扱う
  (Host / Origin 検証は適用)。
- `GET /api/v1/system/info`: 接続先が `hub` か `local_node`
  か、および Web Push の VAPID Public Key を返却。ローカルノード接続時は
  `node_id`（自分のノードID）、`unsynced_event_count`（Outbox 残数）、
  `hub_connected`（ハブ接続状態）も返却。UI はこの `node_id` を
  `/api/v1/nodes/{node_id}/...` のパスに使う。
- `GET /healthz` / `GET /readyz`: 認証不要の liveness / readiness。
  `readyz` は DB クエリが成功した場合のみ 200、失敗時は 503 を返す。
- `GET /api/v1/diagnostics`: 診断情報 (`DiagnosticsResponse`)。エージェントごとの
  直近 stderr (`agents[].stderr_tail`、最大 200 行)、`node_log` のパス、
  コンポーネントヘルス / ドロップカウンタ (`health: SystemStatus`)、
  直近の未解決インシデント (`incidents`) を返す。ハブでは
  `agents=[]` を返し、`health` は `db` / `migrations` / `http` / `hub` /
  `disk` を報告する。
- `GET /api/v1/diagnostics/incidents?limit=50&include_resolved=false`:
  永続化されたインシデントの一覧 (`IncidentListResponse`)。新しい順。
  `limit` は最大 500。`resolved_at` が未設定のものだけが既定の対象。
- `POST /api/v1/diagnostics/incidents/:id/resolve`: インシデントを解決済みに
  する (204)。既に解決済みでも成功扱い。
- `GET /api/v1/diagnostics/bundle`: サポートバンドル (`SupportBundle`)。
  設定 (`config.toml` の安全な項目)、DB 統計、ヘルス、インシデント、
  セッション概要、ログ末尾を含み、トークン / Cookie / プロンプト /
  ホームディレクトリは**必ず redaction される**。`fxg support bundle` と
  Web UI の「Export support bundle」が同じエンドポイントを使う。
- REST のエラー応答には `x-fxg-error-code` ヘッダが付く。診断ミドルウェアは
  このヘッダだけを読み、5xx と復旧可能な 409 / 408 / 429 をインシデントとして
  記録する (レスポンスボディは読まない)。
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
  (Web UI の Worktree 整理。`{ node_id }`)。
- `POST /api/v1/nodes/:node_id/projects/scan`:
  指定ディレクトリ (省略時は `config.toml` の `node.project_scan_dirs`)
  配下の Git リポジトリを一括探索し、論理プロジェクトとして登録する
  (Web UI のプロジェクトスキャン)。`{ dir: Option<String> }` を受け取り、
  `ProjectScanResponse` (`scanned_dirs` + ノード上での登録プロジェクト一覧)
  を返す。
- `POST /api/v1/nodes/:node_id/projects/link`:
  任意ディレクトリを論理プロジェクトへ手動で紐付ける
  (Web UI のプロジェクト紐付け)。`{ project_id, local_path }` の
  `project_id` には `project_key` (`user:` プレフィックス可) を渡し、
  対象ノードの `node.db.project_path_links` を更新する。Worktree の場合は
  メイン Worktree のルートにリンクする。既存セッションの `sessions.project_id`
  も新しい `user:<key>` へ付け替える。
- `POST /api/v1/nodes/:node_id/projects/unlink`:
  指定ノード上のフォルダ紐付け (`project_node_bindings`) を削除する
  (Web UI のプロジェクト削除)。`{ local_path }` を受け取り、最後の紐付け
  だった論理プロジェクトは破棄する（セッションは保持）。未登録のパスは 404。
- `GET /api/v1/nodes`:
  ノード一覧とオンライン状態、一時ノード属性（`is_ephemeral`,
  `lifecycle_status`）、利用可能エージェント一覧を返却。
- 設定 (すべて型付き構造体。汎用 Key-Value は使わない):
  - `GET /api/v1/nodes/:node_id/settings`: `NodeSettingsSnapshot`
    (`pairing: PairingSettings` を含む。トークン値は返さない)
  - `PATCH /api/v1/nodes/:node_id/settings/node` / `.../agents`:
    ノード所有設定の更新
  - `PUT /api/v1/nodes/:node_id/settings/pairing`:
    ハブ接続先 (`hub_url`) とトークンを保存し、即時再接続する
    (トークン省略は接続先が同じ場合のみ。別ハブへは `AUTH_REQUIRED`)
  - `DELETE /api/v1/nodes/:node_id/settings/pairing`:
    ペアリングを解除する (`{ expected_revision }`。outbox は保持)
  - `POST /api/v1/nodes/:node_id/settings/pairing/test`:
    保存せずに WebSocket ハンドシェイクだけ試し、`HubLinkStatus` を返す
  - `GET/PATCH /api/v1/settings/hub`: `HubSettingsSnapshot` (ハブ)
  - ローカルノードも自身の `node_id` で同じ `/nodes/{node_id}/...` を使う
    （`GET /api/v1/system/info` の `node_id` で取得する）。
  - 更新は `expected_revision` による楽観的ロック。競合は `REVISION_MISMATCH`。
- 秘密 (値は書き込み専用。レスポンスは `SecretsResponse` = メタデータのみ):
  - ノード所有: `GET /api/v1/nodes/:node_id/secrets`,
    `PUT/DELETE /api/v1/nodes/:node_id/secrets/:secret_id`
  - ハブ所有: `GET /api/v1/hub/secrets`,
    `PUT/DELETE /api/v1/hub/secrets/:secret_id`
  - 参照中の削除は `SECRET_IN_USE`、値ファイル欠損は `SECRET_MISSING`。
- `GET /api/v1/nodes/:node_id/agents`: 指定ノードの ACP カタログ
  (`GET /api/v1/agents` はローカルノード専用。ハブは代表ノードを選ばない)。
- `POST /api/v1/nodes/:node_id/agents/:agent_id/probe`:
  新規セッション画面用の能力プローブ。body は
  `{ local_path, force? }`（`force: true` でキャッシュを迂回する）。
  ACP の `session/new` を1回だけ実行して `configOptions` (モデル一覧など) と
  `available_modes` を `AgentCapabilitiesPreview` として返し、直後に
  セッションを閉じる。FXG の `sessions` 行は作らない。ノードは
  `(agent_id, local_path)` 単位で約10分キャッシュし、同時実行は直列化する。
  認証が必要な場合は通常の `AUTH_REQUIRED` を返す。
- `GET /api/v1/provisioners`:
  利用可能な一時VM・サンドボックスプロビジョナー一覧（`local-docker`,
  `local-incus`, `colab-pro` 等）を返却。
- `POST /api/v1/provisioners/:name/test`:
  指定プロビジョナーの起動と `fxg internal stdio` の `NodeHello`
  ハンドシェイク疎通を検証する（ハブのみ。プロビジョナーはハブホスト上で
  子プロセス起動されるため）。`ProvisionerTestResponse`
  (`ok` / `node_id` / `stderr` ログ末尾 / `error`)
  を返却し、検証後は一時環境を破棄する。
- `GET /api/v1/sessions?project_id=...&status=...&include_archived=true`:
  セッション一覧。削除済み (tombstone) は常に除外され、アーカイブ済みは
  `include_archived=true` 指定時のみ返却される (既定は除外)。
- `POST /api/v1/sessions`: 新規セッションの開始（既存の常駐 `node_id`
  指定のほか、`provisioner`
  指定による一時VMのオンデマンド起動、Worktreeパス指定、
  同一ノード・同一エージェントのACPネイティブForkに対応）。
- `GET /api/v1/sessions/:id`:
  セッション詳細情報（ステータス、設定、参加エージェント、Worktree等）の取得 (`SessionDetail`)。
- `POST /api/v1/sessions/:id/prompt`:
  セッションへのプロンプト送信 (`PromptInput`, `client_source: ClientSource`)。応答は `CommandResult`。
- `POST /api/v1/sessions/:id/control`:
  セッション制御 (`action: SessionControlAction` 例: `abort`, `pause`)。応答は `CommandResult`。
- `POST /api/v1/sessions/import`:
  外部 ACP セッションやトランスクリプトのインポート。応答は `SessionImportResponse`。
- `GET /api/v1/projects/:project_id/worktrees`:
  プロジェクトの Worktree 一覧取得 (`WorktreeListResponse`)。
- `POST /api/v1/projects/:project_id/worktrees`:
  新規 Worktree の作成・追加 (`{ branch, new_branch? }`)。
- `DELETE /api/v1/projects/:project_id/worktrees/:worktree_id`:
  指定 Worktree の削除。
- `GET /api/v1/sessions/:id/events?after_cursor=&before_cursor=&limit=`:
  指定カーソル以降のイベント履歴、または `before_cursor` 指定時はその直前の
  末尾イベントを取得する（`SessionEventBatch` を返却、古い順）。
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
- `POST /api/v1/sessions/:id/resume`:
  停止済みセッションを同一 `session_id` のまま再開する (`fxg session resume`
  の Web UI 版)。エージェント側コンテキストのネイティブ復元
  (`session/resume` → `session/load`) を優先し、
  非対応の場合は履歴 Replay を注入して継続する。応答は `ResumeSessionResponse`
  (`session_id` / `context_restored`)。稼働中セッションは `INVALID_STATE`、
  一時VMセッションは v1 では `INVALID_STATE`
  を返却する。ハブ経由の場合は
  対象ノードへ `ResumeSession` を中継する (設計: docs/04 §4.1)。
- `POST /api/v1/sessions/:id/title`:
  セッション名を変更する (`{ "title": String }`。1〜200 文字、空文字は
  `INVALID_STATE`)。`SessionTitleChanged` を追記するため、ハブ・他クライアント
  にも同じ名前が同期される。応答は `SessionRenameResponse` (`session_id` /
  `title`)。ハブ経由の場合は対象ノードへ `RenameSession` を中継する。
- `POST /api/v1/sessions/:id/archive`:
  セッションをアーカイブ/復元する (`{ "archived": true | false }`。
  `fxg session archive` / `unarchive` の Web UI 版)。アーカイブは
  一覧からの非表示/復元のみで、イベントログは保持される。応答は
  `SessionArchiveResponse` (`session_id` / `archived_at`)。ハブ
  経由の場合は対象ノードへ `ArchiveSession` を中継する。
- `DELETE /api/v1/sessions/:id`:
  セッションを削除する (`fxg session delete` の Web UI 版。204 No Content)。
  稼働中セッションは停止を待ってから `SessionDeleted` (tombstone) を追記し、
  本文イベント・承認履歴をパージする (**復元不能**)。
  アーカイブ済みでも削除できる。ハブ経由の場合は対象ノードへ
  `DeleteSession` を中継する。
- `GET /api/v1/search?q=...&limit=...`: SQLite FTS5 を用いた全セッション
  横断の全文検索。クエリは URL のクエリパラメータのみで受け取り、JSON body は
  受け付けない。
- `POST /api/v1/push/subscribe`: Android / Desktop PWA の Web Push
  サブスクリプション登録 (ハブのみ)。VAPID 鍵は初回起動時に
  `~/.flexagent/vapid.json` へ自動生成され、公開鍵は `GET /api/v1/system/info`
  の `vapid_public_key` (base64url) で配布される。
- `POST /api/v1/system/kill-switch`: **緊急停止 (Panic
  Button)**。全ノードの稼働中セッション、実行中プロセスツリー、PTYを一括強制終了。
- `POST /api/v1/system/shutdown`: ハブを graceful shutdown する（`fxg hub stop` が使用。
  認証必須。再起動は `fxg hub start`）。
- `GET /api/v1/audit/logs?limit=50`:
  監査ログ（操作日時、操作種別、送信元IP、クライアント種別）の取得。ハブでは
  `hub.db`、 ローカルノードでは `node.db` の `audit_logs` を参照する。
- `GET /api/v1/agents`:
  `{ agents, registry }` を返す。`agents` は **builtin + custom** の利用可能な
  エージェント (`AgentSummary[]`) で、`sources` (`external` / `custom`)、
  `external_path`、`custom_config`、`backend` (`acp` / `acp_extended` /
  `custom`)、`icon` を持つ。`sources` が空 = そのノードでは未導入。
  `registry` は ACP Registry の import 候補 (`RegistryAgentSummary[]`) で、
  `distribution` (`binary` / `npx` / `uvx`)、`available`、
  `unavailable_reason`、`imported` を持つ。Registry の取得に失敗、または
  5秒でタイムアウトしても `agents` は返す。
  ハブでは `ListAgents` の結果に全ノードの `NodeHello.installed_agents` を
  統合して `installed_nodes` (利用可能なノード一覧) を埋める。
- `PUT /api/v1/nodes/:node_id/agents/custom/:agent_id`:
  カスタム ACP エージェント定義を作成・更新する (body: `CustomAgentConfig` =
  `{ name, description, command, args, env, env_secrets, extension, icon,
  origin }`)。`extension` は組み込み extension の ID (`opencode` など) で、
  指定すると Tier 2 として起動する。未知の ID と予約 ID は保存時に拒否される。
  `AgentAction::UpsertCustom` として中継される。
- `DELETE /api/v1/nodes/:node_id/agents/custom/:agent_id`:
  UI 管理のカスタム定義を削除する (`AgentAction::RemoveCustom`)。
- `POST /api/v1/nodes/:node_id/agents/registry/:agent_id/import`:
  ACP Registry のエントリを custom 定義として保存する
  (`AgentAction::ImportRegistryAgent`)。body は
  `{ extension: Option<String> }`。binary は `PATH`、npx/uvx はパッケージ
  ランナーを使ってコマンドを生成する。既存 custom・Tier 3 予約 ID・未知の
  extension は拒否する。
- `POST /api/v1/auth/rotate-token`:
  クライアント認証トークン (`auth_token`) を再生成し `{ token }` を返す。
  旧トークン (Bearer / `fxg_session` Cookie) は即時無効化される。
- `GET /api/v1/nodes/tokens`:
  発行済みノード個別トークン一覧 (`node_id` + トークンハッシュ先頭 12 文字。
  平文は保存しない)。ハブのみ。
- `POST /api/v1/nodes/tokens`:
  ノード個別トークンを発行
  (`{ node_id }`)。平文トークンは**発行時の応答でのみ**返却し、
  `hub.db.nodes.token_hash` には SHA-256
  ハッシュを保存する。ハブのみ。
  未登録のノードはプレースホルダで登録し `NodeHello` で上書きされる。
- `DELETE /api/v1/nodes/tokens/:node_id`:
  ノード個別トークンを失効させる。ハブのみ。

### 3.2 Client WebSocket (`/api/v1/client/ws`)

1. 接続時にクライアントが
   `Subscribe { since_cursor: Option<u64>, focused_session_id: Option<String> }`
   を送信する。
   - `since_cursor` は**接続先ストア**（ハブなら
     `hub.db`、ローカルノードなら `node.db`）の `session_events.cursor`
     を指定する。接続先を切り替えた場合は
     もう一方のカーソルは使えないため、`None`（初回全量）から開始する。
2. ハブ / ノードは該当カーソル以降の未取得イベントを
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
3. クライアントからの上りメッセージは `Subscribe` / `SendPrompt` /
   `ControlSession` / `Ping` に限定する。`SendPrompt` と `ControlSession` の
   結果は `command_id` 付きの `CommandResult` として要求元クライアントへ
   応答される。
   - Permission / Elicitation の回答、Resume / Archive / Delete は
     **REST 専用**とする（冪等な状態変更を1経路にまとめ、WS と REST の
     ペイロード差異をなくす）。REST は `already_resolved` を返す。
   - **一時VMプロビジョニングログ (`ProvisioningLog`)**:
     一時VMの起動中（`fxg internal stdio`
     の `NodeHello` 前）はノードの `node.db` が存在しないため、ハブ
     がプロビジョナーの `stderr` を `{ op: "provisioning_log", session_id,
     line }` として**エフェメラル配信**する（イベントログには永続化されず、
     再接続時は復元されない）。UI はセッション詳細の「Provisioner
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
    の場合、リモート（ハブ経由）からの `spawn` 要求に対して
    `{ op: "error", code: "FORBIDDEN", message: "Remote PTY is disabled on this node by security policy" }`
    を返し、接続を切断します。
- クライアントから接続時に `{ op: "attach", pty_id: "..." }` または
  `{ op: "spawn", session_id: "...", cols: 80, rows: 24 }` を送信。
- ユーザーのキー入力は `{ op: "input", data_b64: "..." }`
  で即座にノードのPTY標準入力へ書き込まれます。
- 画面リサイズ時は `{ op: "resize", cols: N, rows: M }` を送信し、ConPTY / Unix
  PTYのウィンドウサイズを動的変更します。

---

## 4. CLI (`fxg`) ⇔ Local Node (`fxg node`) 間 ローカル通信 (HTTP REST / WebSocket)

プラットフォーム固有のローカルIPC（Windows Named Pipe / Unix Domain Socket）および長さプレフィックス付き独自バイナリ/JSONフレーミングは**全廃**され、ローカルノードのループバック HTTP/WS サーバーに一本化されました。

これにより、CLI (`fxg`)、PC ブラウザ、スマホ PWA のすべてが同一の REST API と WebSocket プロトコルを共有し、重複したルーティング・メッセージ定義・保守コストが完全に排除されます。

### 4.1 接続先・認証・起動待機

- **接続先**:
  - ローカルノード: `node.addr` に記録された `http://127.0.0.1:<port>`。既定は `127.0.0.1:7860`。
    `AddrInUse` かつ `[node] listen_addr` / `--listen` が明示されていない場合のみ
    ephemeral port にフォールバックし、実際のアドレスを `node.addr` に書く。
  - ハブへ直接接続する場合（`--provisioner` / `fxg web hub` 等）:
    `--hub` → `hub.addr` → ローカルノードのペアリング (`pairing.hub_url`) →
    `[hub] listen_addr` の順で解決する。
  - `FXG_NODE_URL` のような CLI 専用 URL 環境変数は設けない。`FXG_HUB_URL` と
    `FXG_NODE_TOKEN` は廃止し、ペアリングは `node.db` が唯一の保存先になった。
- **認証**:
  - `~/.flexagent/auth_token` (`0600`) のトークンを読み、
    `Authorization: Bearer <AUTH_TOKEN>` を付与する。
  - `GET /healthz` / `GET /readyz` / `GET /api/v1/meta` は認証免除。
- **未起動・stale addr**:
  - `node.addr` を読み、`GET /healthz` で疎通確認する。失敗時は
    `fxg node start --detach` 相当で自動起動し、addr を再読込する。
  - `FXG_NO_AUTO_NODE=1` では自動起動しない。
- **バージョン**:
  - `GET /api/v1/system/info` の `version` を確認し、旧ノードなら
    `fxg node restart` を案内する。ノード⇔ハブは `PROTOCOL_VERSION` で検証する。

### 4.2 CLI コマンドと REST / WS の対応

| CLI コマンド                            | 使用する REST / WS                                                       | 説明                                                        |
| :-------------------------------------- | :----------------------------------------------------------------------- | :---------------------------------------------------------- |
| `fxg session new` / `-w`                | `POST /api/v1/nodes/:id/projects/inspect` → `POST /api/v1/sessions`      | cwd から `project_id` を解決。`-w` は `worktree` フィールド |
| `fxg session list`                      | `GET /api/v1/sessions`                                                   | セッション一覧                                              |
| `fxg session show`                      | `GET /api/v1/sessions/:id` + `GET /api/v1/sessions/:id/events?limit=...` | 詳細と末尾イベント                                          |
| `fxg session prompt`                    | `POST /api/v1/sessions/:id/prompt`                                       | dispatch 後に `CommandResult`。`--wait` は WS 購読          |
| `fxg session stop` / `cancel` / `kill`  | `POST /api/v1/sessions/:id/control`                                      | `SessionControlAction`                                      |
| `fxg session fork`                      | `POST /api/v1/sessions`（`fork` 指定）                                   | ACP ネイティブ Fork                                         |
| `fxg session resume`                    | `POST /api/v1/sessions/:id/resume`                                       | 停止セッションの再開                                        |
| `fxg session archive`                   | `POST /api/v1/sessions/:id/archive`                                      | アーカイブ / 復元                                           |
| `fxg session delete`                    | `DELETE /api/v1/sessions/:id`                                            | セッション削除                                              |
| `fxg session diff`                      | `GET /api/v1/sessions/:id/diff`                                          | 作業ツリー差分                                              |
| `fxg session import`                    | `POST /api/v1/sessions/import`                                           | 外部セッションのインポート                                  |
| `fxg node status`                       | `GET /api/v1/system/info` + `GET /api/v1/sessions`                       | 待受アドレス / hub 接続状態 / Outbox / 稼働セッション       |
| `fxg node stop`                         | `POST /api/v1/system/shutdown`                                           | ローカルノードの graceful shutdown                          |
| `fxg node access-token show` / `rotate` | `auth_token` ファイル読み取り / `POST /api/v1/auth/rotate-token`         | トークン表示・再生成                                        |
| `fxg node pair` / `fxg node unpair`     | （API なし）                                                             | `node_token` と `[node] hub_url` の一括設定・解除           |
| `fxg support bundle` / `incidents`      | `GET /api/v1/diagnostics/bundle` / `GET /api/v1/diagnostics/incidents`   | サポートバンドル / 障害履歴                                 |
| `fxg doctor` / `fxg logs --session`     | `GET /api/v1/diagnostics`                                                | ローカル診断 / エージェント stderr                          |
| `fxg agents login` / `logout`           | （API なし）                                                             | ローカルプロセスで実行                                      |
| リアルタイム出力 / `--wait`             | `/api/v1/client/ws`                                                      | `Subscribe` / `EventBatch` / `LiveStreamDelta`              |
| `fxg worktree ...`                      | （独立コマンドなし）                                                     | `session new -w` / `fork -w` の内部処理                     |
| `fxg inbox`                             | （独立コマンドなし）                                                     | Web UI が `/api/v1/inbox` を使用                            |

### 4.3 追加する REST エンドポイントの契約

- `GET /api/v1/sessions/:id?recent_events=20`:
  セッション詳細 (`SessionDetail`) を返す。`recent_events` は末尾イベント数
  （既定 20、最大 500）。型は `fxg-protocol/src/client_api.rs` へ移設済み。
- `POST /api/v1/sessions/:id/prompt`:
  body は `{ "command_id": "...", "input": PromptInput, "client_source": ClientSource }`。
  dispatch 後に `CommandResult` を返す。ターン完了は待たず、`--wait` は WS で判定する。
- `POST /api/v1/sessions/:id/control`:
  body は `{ "command_id": "...", "action": SessionControlAction }`。応答は `CommandResult`。
- `POST /api/v1/sessions/import`:
  body は `{ "command_id": "...", "node_id": ..., "agent_id": "...", "agent_session_id": "...", "cwd": "...", "title": ... }`。
  ローカルノードは `cwd` から解決し、`node_id` は省略可（指定時は自ノードと照合）。
  ハブは `node_id` 必須で対象ノードへ RPC 中継する。応答は
  `{ "command_id": "...", "session_id": "..." }`。
- `GET /api/v1/sessions/:id/events?after_cursor=&limit=&before_cursor=`:
  `SessionEventBatch` を返す。`limit` / `before_cursor` は `session show` 用の追加。
- `POST /api/v1/system/shutdown`:
  ローカルノードでは `DaemonState::request_shutdown` を実装し `{ "ok": true }` を返す。
- エラーは既存の `ApiErrorResponse` (`{ "error": { "code", "message", ... } }`) に統一する。

### 4.4 廃止された IPC 関連コンポーネント

- `crates/fxg-protocol/src/ipc.rs`（`IpcRequest`, `IpcRequestKind`, `LocalRequest`, `IpcResponse` 等）
- `crates/fxg-node/src/daemon/ipc.rs`（Windows Named Pipe / Unix Domain Socket サーバー）
- `crates/fxg-node/src/ipc_client.rs` / `crates/fxg-node/src/ipc_framing.rs`
- `crates/fxg-cli/src/client.rs` の IPC クライアント（HTTP / WS クライアントへ書き換え）
- `fxg internal git-askpass` 用のローカル IPC リレー機構
- `DaemonConfig.ipc_endpoint`、`FXG_IPC_ENDPOINT`、テスト用 IPC エンドポイント、CLI の IPC 表示
