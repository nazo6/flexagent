# 03. プロトコル・API・IPC仕様 (`fxg-protocol`)

`crates/fxg-protocol` クレートに定義する共通データ型と、3つの通信経路（① Node ⇔
Server WebSocket、② Client ⇔ Server/Node HTTP+WS、③ CLI ⇔ Node Local
IPC）のプロトコル仕様です。 すべての構造体に
`#[derive(Serialize, Deserialize, ts_rs::TS)]` を付与し、`ui/src/generated/`
へTypeScriptの型定義を自動出力します。

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
    pub event_id: String,       // UUID v7
    pub session_id: String,     // UUID v7
    pub node_seq: u64,          // セッション内連番
    pub global_seq: Option<u64>,// 中央サーバー採番 (ローカル直結時は None)
    pub created_at: i64,        // Unix epoch ms
    pub payload: UnifiedEventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[ts(export)]
pub enum UnifiedEventPayload {
    /// ユーザーが送信したプロンプト（スラッシュコマンド含む）
    UserMessage {
        text: String,
        attachments: Vec<AttachmentMeta>,
        client_source: String, // "cli" | "web" | "android"
    },
    /// エージェントの返答メッセージ（チャンク結合済み、またはストリーム中）
    AgentMessage {
        message_id: String,
        text: String,
        is_complete: bool,
    },
    /// エージェントの思考プロセス (Thinking)
    AgentThought {
        thought_id: String,
        text: String,
        is_complete: bool,
    },
    /// ツール呼び出しとファイルDiff等の状態
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
    /// ACP terminal/* または PTY の出力チャンク
    TerminalOutput {
        terminal_id: String,
        command: String,
        data_b64: String,      // ANSIエスケープを含む生バイト列 (Base64)
        exit_code: Option<i32>,
    },
    /// モード・スラッシュコマンド・設定の更新通知
    CapabilitiesUpdated {
        current_mode: Option<String>,
        available_modes: Vec<ModeInfo>,
        available_commands: Vec<CommandInfo>,
        config_options: Vec<ConfigOptionInfo>,
    },
    /// セッション状態の変化
    StatusChanged {
        status: SessionStatus,
        error_message: Option<String>,
    },
}
```

---

## 2. Node ⇔ Central Server 間 WebSocket プロトコル

- **エンドポイント**: `wss://<server-host>/api/v1/node/ws`
- **認証**: `Authorization: Bearer <FXG_NODE_TOKEN>` ヘッダ

```rust
/// Node -> Server への送信メッセージ
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NodeToServerMsg {
    /// 接続直後のハンドシェイク（ノード情報・プロジェクト紐付け・アクティブセッション一覧を通知）
    NodeHello {
        node_id: String,
        name: String,
        os: String,
        arch: String,
        version: String,
        installed_agents: Vec<String>,
        projects: Vec<NodeProjectReport>,
    },
    /// セッションメタデータの作成・更新通知
    SessionUpsert {
        session: SessionSummary,
    },
    /// 永続化イベントのバッチ送信 (Store-and-Forward Outbox)
    EventBatchPush {
        events: Vec<SessionEventEnvelope>,
    },
    /// リアルタイム・エフェメラルチャンク (DBフラッシュ前の高速表示用)
    LiveStreamDelta {
        session_id: String,
        delta: StreamDeltaPayload,
    },
    /// サーバーからのコマンド実行結果応答
    CommandResult {
        command_id: String,
        success: bool,
        error: Option<String>,
        session_id: Option<String>,
    },
}

/// Server -> Node への送信メッセージ
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ServerToNodeMsg {
    /// EventBatchPush に対する永続化完了ACK
    EventBatchAck {
        session_id: String,
        acked_up_to_node_seq: u64,
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
}
```

---

## 3. Client (PWA / Browser) ⇔ Server (または Local Node) API

中央サーバー (`fxg server`) と各ノードのローカルWebサーバー (`fxg daemon` on
`localhost:7860`)
は**完全に同一のAPIパスとWebSocketフォーマット**を実装します。これにより、PWAフロントエンドは接続先URLを意識せずにどちらにも繋がります。

### 3.1 REST API エンドポイント

- `GET /api/v1/system/info`: 接続先が `central_server` か `local_node`
  か、および Web Push の VAPID Public Key を返却。
- `GET /api/v1/projects`:
  プロジェクト一覧と、各プロジェクトに紐づくノード（`project_node_bindings`）を返却。
- `GET /api/v1/nodes`:
  ノード一覧とオンライン状態、利用可能エージェント一覧を返却。
- `GET /api/v1/sessions?project_id=...&status=...`: セッション一覧。
- `POST /api/v1/sessions`:
  新規セッションの開始（または既存セッションを別ノードへContext Fork）。
- `GET /api/v1/sessions/:id/events?after_seq=0`:
  指定シーケンス以降のイベント履歴取得。
- `GET /api/v1/inbox`: 全セッション横断の未解決 `PermissionRequest` 一覧。
- `POST /api/v1/search?q=...`: SQLite FTS5 を用いた全セッション横断の全文検索。
- `POST /api/v1/push/subscribe`: Android / Desktop PWA の Web Push
  サブスクリプション登録。

### 3.2 Client WebSocket (`/api/v1/client/ws`)

1. 接続時にクライアントが
   `Subscribe { last_global_seq: Option<u64>, focused_session_id: Option<String> }`
   を送信。
2. サーバーは `last_global_seq`
   以降の未取得イベントを即座に流し、以降はリアルタイムイベント（`SessionEventEnvelope`
   および `LiveStreamDelta`）をプッシュします。
3. クライアントからの操作（`SendPrompt`, `RespondPermission`,
   `ControlSession`）もこのWebSocket上（またはREST POST）で送信可能です。

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
   - `fxg opencode` 実行時に呼ばれ、デーモン側でセッションを開始します。
   - `attach_mode` には以下のいずれかが返ります：
     - `AcpTui`: `fxg` CLI自身の内蔵TUI (`ratatui`)
       でIPCストリームを描画するモード。
     - `NativeOpenCodeAttach { server_url: String, session_id: String }`:
       デーモンが管理する `opencode2 serve` に対して、CLI側が
       `opencode2 run --attach <server_url> --session <id>`
       を子プロセス実行して純正TUIを直接表示するモード。
2. `AttachSession { session_id }`:
   - 既存セッションのイベントストリーム購読＋双方向操作（プロンプト送信・承認応答・リサイズ通知）。
3. `GetLocalStatus`:
   - ローカルで稼働中のセッション一覧、中央サーバーとのWebSocket接続状態、未送信Outboxイベント数を返却。
