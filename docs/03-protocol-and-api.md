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
    /// ハブ側 sessions.current_mode / available_commands_json / config_options_json 投影の更新源
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
    /// セッション状態の変化 ('provisioning' | 'bootstrapping' | 'idle' | 'running' | ...)
    /// ハブ側 sessions.status 投影の更新源
    StatusChanged {
        status: SessionStatus,
        error_message: Option<String>,
    },
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
    pub last_node_seq: u64,
    pub created_at: i64,
    pub updated_at: i64,
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
pub enum SessionStatus { Provisioning, Bootstrapping, Idle, Running, WaitingPermission, Stopped, Error }

#[serde(tag = "action", rename_all = "snake_case")]
pub enum SessionControlAction {
    SetMode { mode_id: String },
    SetConfig { key: String, value: serde_json::Value },
    Cancel,
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
    /// モード切替 / 設定変更 / キャンセル
    ControlSession {
        command_id: String,
        session_id: String,
        action: SessionControlAction, // SetMode(String) | SetConfig(k, v) | Cancel | Kill
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
        action: WorktreeAction,        // Add { branch, new_path } | Remove { path }
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
   - 未認証リクエストは即座に `401 Unauthorized` を返却。
2. **Host ヘッダ検証 (DNS Rebinding 防御)**:
   - リクエストの `Host` ヘッダが `localhost:<port>`, `127.0.0.1:<port>`,
     またはサーバー設定の許可ホスト（例: Tailscale MagicDNS名 /
     LANホスト名）に一致しない場合、`403 Forbidden` を返却。
3. **Origin ヘッダ検証 (Cross-Site WebSocket Hijacking 防御)**:
   - WebSocketハンドシェイク時、`Origin`
     が自サーバーのドメインまたはローカルオリジン以外からの接続である場合、ハンドシェイクを拒否。
4. **監査ログ記録 (Audit Logging)**:
   - `session` 起動、`permission` 解決、`kill-switch` 実行、`worktree`
     操作は、クライアントIP・UA・トークンIDとともに `audit_logs` に記録。
   - 記録先は中央サーバー経由の操作が
     `server.db`、ローカル直結（`localhost:7860` / CLI）の操作が 実行ノードの
     `node.db`（どちらも `/api/v1/audit/logs` で参照可能）。
5. **エラーレスポンスの共通形式**:
   - REST / WS のエラーは
     `{ "error": { "code": "<ErrorCode>", "message": "..." } }` に統一 （例:
     `NODE_OFFLINE`, `ALREADY_RESOLVED`, `PTY_DISABLED`）。

### 3.1 REST API エンドポイント

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
- `GET /api/v1/nodes`:
  ノード一覧とオンライン状態、一時ノード属性（`is_ephemeral`,
  `lifecycle_status`）、利用可能エージェント一覧を返却。
- `GET /api/v1/provisioners`:
  利用可能な一時VM・サンドボックスプロビジョナー一覧（`local-docker`,
  `local-incus`, `colab-pro` 等）を返却。
- `GET /api/v1/sessions?project_id=...&status=...`: セッション一覧。
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
- `GET /api/v1/inbox`: 全セッション横断の未解決 `PermissionRequest` 一覧。
- `POST /api/v1/sessions/:id/permissions/:req_id/respond`:
  承認リクエストへの応答 (`selected_option_id`, `always`,
  `resolved_by`)。既に解決済みの場合は `ALREADY_RESOLVED` を返却し（冪等）、UI
  側は正常遷移として扱う。
- `POST /api/v1/search?q=...`: SQLite FTS5 を用いた全セッション横断の全文検索。
- `POST /api/v1/push/subscribe`: Android / Desktop PWA の Web Push
  サブスクリプション登録。
- `POST /api/v1/system/kill-switch`: **緊急停止 (Panic
  Button)**。全ノードの稼働中セッション、実行中プロセスツリー、PTYを一括強制終了。
- `GET /api/v1/audit/logs?limit=50`:
  監査ログ（操作日時、操作種別、送信元IP、クライアント種別）の取得。中央サーバーでは
  `server.db`、 ローカルノードでは `node.db` の `audit_logs` を参照する。

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
3. クライアントからの操作（`SendPrompt`, `RespondPermission`,
   `ControlSession`）もこのWebSocket上（またはREST POST）で送信でき、結果は
   `command_id` 付きの `CommandResult` として要求元クライアントへ応答されます。
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

1. `EnsureSession { cwd, agent_id, extra_args } -> { session_id, attach_mode }`:
   - `fxg run opencode` 実行時に呼ばれ、デーモン側でセッションを開始します。
   - `attach_mode` には以下のいずれかが返ります：
     - `AcpTui`: `fxg` CLI自身の内蔵TUI (`ratatui`)
       でIPCストリームを描画するモード。
     - `NativeOpenCodeAttach { server_url: String, session_id: String }`:
       デーモンが管理する `opencode2 serve` に対して、CLI側が
       `opencode2 run --attach <server_url> --session <id>`
       を子プロセス実行して純正TUIを直接表示するモード。
2. `AttachSession { session_id, after_node_seq: Option<u64> }`:
   - 既存セッションのイベントストリーム購読＋双方向操作（プロンプト送信・承認応答・リサイズ通知）。
     `after_node_seq`
     指定時はその連番以降の履歴をリプレイしてからライブストリームへ接続する（途中切断からの再接続用）。
3. `GetLocalStatus`:
   - ローカルで稼働中のセッション一覧、中央サーバーとのWebSocket接続状態、未送信Outboxイベント数を返却。
