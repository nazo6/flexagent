# API・プロトコル統合リファクタリング計画

- **日付**: 2026-11-09
- **ステータス**: 計画中 (Proposed)
- **対象パッケージ**: `fxg-protocol`, `fxg-server`, `fxg-node`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**:
  `mise run fmt` / `mise run lint` / `mise run check` / `mise run types` /
  `cargo test --workspace` / `pnpm --dir ui run check` / `pnpm --dir ui run test`
- **関連ドキュメント**:
  - [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md)
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md)
  - [`docs/06-type-safety.md`](../06-type-safety.md)
  - [`docs/changelog/2026-11-09-codebase-refactoring-plan.md`](2026-11-09-codebase-refactoring-plan.md)

---

## 0. 背景と課題

機能追加（設定管理、秘密ストア、FSブラウズ、Worktree操作、エージェント管理など）が進むにつれ、1つの操作を追加するたびに **3つの異なる通信境界・計7〜8箇所のファイル** にボイラープレートコードを書く構造になっている。

### 現状の「1機能追加あたりの変更箇所」

1. **Client ⇔ Server/Node REST API (`fxg-protocol` / `fxg-server` / `fxg-node` / `ui`)**
   - `crates/fxg-protocol/src/client_api.rs` に Request / Response 構造体を追加
   - `crates/fxg-server/src/api/mod.rs` の `ClientApiBackend` トレイト（現在約40メソッド）にメソッドを追加
   - `crates/fxg-server/src/api/mod.rs` の `client_router` に Axum ルートとハンドラ関数を追加
   - `crates/fxg-node/src/daemon/api.rs`（LocalNode用）と `crates/fxg-server/src/state.rs`（CentralServer用）の両方に実装を追加
   - `ui/src/lib/api/client.ts` の `ApiClient` に手書きの fetch ラッパーメソッドを追加
2. **Server ⇔ Node 間リレープロトコル (`fxg-protocol` / `fxg-server` / `fxg-node`)**
   - `crates/fxg-protocol/src/node_server.rs` の `ServerToNodeMsg` にリクエストバリアント、`NodeToServerMsg` に専用の `*Result` バリアントを追加
   - `crates/fxg-server/src/hub.rs` の `PendingResponse` enum（現在13バリアント）と専用リレーメソッドを追加
   - `crates/fxg-node/src/sync.rs` の `handle_server_message` に分岐を追加
3. **CLI ⇔ Daemon ローカル IPC (`fxg-protocol` / `fxg-node` / `fxg-cli`)**
   - `crates/fxg-protocol/src/ipc.rs` の `IpcClientMessage` / `IpcResult` に別定義を追加
   - `crates/fxg-node/src/daemon/ipc.rs` と `crates/fxg-cli/src/client.rs` に実装を追加

### 具体的な問題点

| #   | 問題                                                      | 該当箇所                                                                          | 影響                                                                                                                                                                                                                                                                  |
| :-- | :-------------------------------------------------------- | :-------------------------------------------------------------------------------- | :-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | **NodeHub リレーの個別定義とエラー情報の欠落**            | `crates/fxg-protocol/src/node_server.rs`<br>`crates/fxg-server/src/hub.rs:72-760` | RPC ごとに `*Result` と `PendingResponse` が増え、一部（`GitDiffResult`, `BrowseFsResult`, `ProjectScanResult`, `AgentsResult` 等）は `error: Option<String>` しか持たないため、ノード側の `ErrorCode` や `RecoveryAction` が Hub で `400 Bad Request` に潰れている。 |
| 2   | **LocalNode 用と Hub Proxy 用の重複 REST エンドポイント** | `crates/fxg-server/src/api/mod.rs:714-902`<br>`ui/src/lib/api/client.ts:466-612`  | 設定・秘密・エージェント一覧に「ローカル直叩き用（`/api/v1/settings/node` 等）」と「ノード指定用（`/api/v1/nodes/{node_id}/settings` 等）」の2系統が存在し、`ClientApiBackend` に `InvalidState` を返すだけのスタブメソッドが大量発生している。                       |
| 3   | **REST と Client WS のコマンド重複とシグネチャ乖離**      | `crates/fxg-protocol/src/client_api.rs:438-446, 620-630`                          | `RespondPermission` が REST と WS の両方にあるが、REST 側（`RespondPermissionRequest`）にのみ `always: bool` フィールドが存在し、WS 側（`ClientWsMessage::RespondPermission`）には存在しない。                                                                        |
| 4   | **`ClientApiBackend` トレイトの肥大化**                   | `crates/fxg-server/src/api/mod.rs:496-953`                                        | 中央サーバー専用機能（Provisioner、Web Push、Node Token、Project Profile、Server Settings）とノード操作が1つのトレイトに混在し、約40メソッドに膨張している。                                                                                                          |
| 5   | **CLI IPC と Client API / Node RPC の型分離**             | `crates/fxg-protocol/src/ipc.rs`<br>`crates/fxg-node/src/daemon/ipc.rs`           | 同じ `DaemonState` に対する操作であるにもかかわらず、IPC 専用の Request / Result 型とディスパッチャを別途維持している。                                                                                                                                               |

---

## 1. アーキテクチャ方針の選択肢と比較

Client ⇔ Server/Node 間の API 方式について、以下の2つのアプローチを比較する（gRPC / GraphQL / `rspc` 等の外部 RPC フレームワーク導入は、単一バイナリ構成・`specta` による型生成・既存の Cookie/Host/Origin セキュリティミドルウェアとの相性が悪いため採用しない）。

| 観点           | 方式 A: REST 維持 + ノードスコープ統合 (推奨: 段階的移行)                                                                                                           | 方式 B: 型付き Enum RPC (`POST /api/v1/rpc`) への集約                                                                                                                                            |
| :------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------ | :----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **概要**       | HTTP/REST のパス体系を維持しつつ、重複しているローカル/リモート API を `/api/v1/nodes/{node_id}/...` に一本化し、内部の Node 実行・中継を `NodeRpc` で共通化する。  | `/healthz`, `/api/v1/meta`, `/api/v1/auth/*`, `/api/v1/*/ws` 以外の全操作を `POST /api/v1/rpc`（または `/api/v1/rpc/{op}`）の単一ディスパッチに集約する。                                        |
| **メリット**   | ・既存の REST URL 設計、HTTP ステータス、ブラウザ DevTools での視認性を維持できる<br>・外部スクリプトや `curl` からの利用が容易<br>・段階的にリファクタリングできる | ・Axum のルート定義（約40個）とハンドラ関数がほぼ消滅する<br>・`specta` で `ApiRequest` / `ApiResponse` を出力すれば、`ui/src/lib/api/client.ts` の手書きメソッドも汎用 `call()` 1つに削減できる |
| **デメリット** | ・新しい REST エンドポイントを足す際に Axum ルートと `ApiClient` メソッドを1つずつ書く手間自体は残る                                                                | ・HTTP メソッド（GET/POST/PUT/DELETE）の意味論を使わなくなる<br>・UI 側の呼び出し箇所を一括変更する必要がある                                                                                    |

> **推奨アプローチ**:
> まず **Phase 1〜3（`NodeRpc` 汎用化・エラー統一・REST 重複エンドポイントの統合・WS/REST 重複整理）** を実施する。これだけで `NodeHub`・`ClientApiBackend`・`ApiClient` のボイラープレートの大半が解消される。その上で、さらにルート定義自体を無くしたい場合に **Phase 4（Client RPC エンドポイント化）** へ進む二段構えとする。

---

## 2. リファクタリング詳細設計

### 2.1 Phase 1: Server ⇔ Node 間 (`NodeHub`) の汎用 RPC エンベロープ化

`ServerToNodeMsg` / `NodeToServerMsg` を「ストリーム・制御メッセージ」と「汎用 RPC（Request / Response）」に分離する。

#### 変更後のプロトコル構造 (`crates/fxg-protocol/src/node_server.rs`)

```rust
/// サーバーからノードへのメッセージ
pub enum ServerToNodeMsg {
    // --- 同期・ストリーム・ライフサイクル制御 ---
    EventBatchAck { session_id: SessionId, acked_up_to_node_seq: u64 },
    ResyncRequest { sessions: Vec<ResyncTarget> },
    PtySpawn { pty_id: String, session_id: SessionId, cols: u16, rows: u16, shell_cmd: Option<String> },
    PtyInput { pty_id: String, data_b64: String },
    PtyResize { pty_id: String, cols: u16, rows: u16 },
    PtyKill { pty_id: String },
    GitCredentialResponse { request_id: String, username: String, token: Option<String>, error: Option<String> },
    DrainAndShutdown { reason: String },
    KillAllSessions { reason: String },

    // --- 汎用 Node RPC リクエスト ---
    NodeRpc {
        request_id: CommandId,
        request: NodeRpcRequest,
    },
}

/// ノードからサーバーへのメッセージ
pub enum NodeToServerMsg {
    // --- 同期・ストリーム・ライフサイクル報告 ---
    NodeHello { /* 既存フィールド */ },
    EventBatchPush { events: Vec<SessionEventEnvelope> },
    LiveStreamDelta { session_id: SessionId, delta: StreamDeltaPayload },
    PtyOutput { pty_id: String, data_b64: String },
    PtyExit { pty_id: String, exit_code: Option<i32> },
    PtyError { pty_id: String, code: String, message: String },
    GitCredentialRequest { request_id: String, repo_url: String, operation: String },
    DrainComplete { node_id: NodeId },

    // --- 汎用 Node RPC レスポンス（エラーは常に構造化 ErrorInfo） ---
    NodeRpcResult {
        request_id: CommandId,
        result: Result<NodeRpcResponse, ErrorInfo>,
    },
}
```

#### `NodeRpcRequest` / `NodeRpcResponse` の定義

これまで個別のメッセージバリアントだった以下の操作をすべて `NodeRpcRequest` に統合する：

- セッション操作: `StartSession`, `SendPrompt`, `RespondPermission`, `RespondElicitation`, `ControlSession`, `ResumeSession`, `ArchiveSession`, `DeleteSession`
- ワークスペース・Git 操作: `GetGitDiff`, `ManageWorktree`, `BrowseFs`, `ProjectScan`, `ProjectLink`
- エージェント・設定・秘密操作: `ListAgents`, `ManageAgent`, `GetNodeSettings`, `UpdateNodeSettings`, `UpdateAgentSettings`, `GetSecrets`, `UpsertSecret`, `DeleteSecret`, `RotateSecret`

#### 得られる効果

- `crates/fxg-server/src/hub.rs` の `PendingResponse` enum（13バリアント）を `Result<NodeRpcResponse, ErrorInfo>` の **1種類のみ** に削減できる。
- `hub.rs` の個別リレーメソッド（`git_diff`, `worktree_op`, `resume_session`, `archive_session`, `delete_session`, `browse_fs`, `scan_projects`, `link_project`, `agents`, `manage_agent`, `node_settings`, `secret_op`）を **`hub.rpc(node_id, request)` の1メソッド** に統合でき、約350行の重複コードが消える。
- `NodeRpcRequest::timeout(&self) -> Duration` メソッドで操作種別ごとのタイムアウト（通常30秒、セッション起動・再開120秒、エージェント導入600秒）を一元管理できる。
- すべてのノード操作で `NodeError::info()` から生成した `ErrorInfo`（`ErrorCode`, `RecoveryAction`, `DataSafety`）が欠落せずクライアントへ返る。

---

### 2.2 Phase 2: Client REST API の重複エンドポイント統合と `ClientApiBackend` 整理

#### 1. ローカル専用パスとノード指定パスの統一

現在2系統に分かれているノード所有リソースの API を `/api/v1/nodes/{node_id}/...` に一本化する。

| 廃止するローカル専用エンドポイント          | 統一後のエンドポイント                             | 備考                                                                      |
| :------------------------------------------ | :------------------------------------------------- | :------------------------------------------------------------------------ |
| `GET /api/v1/settings/node`                 | `GET /api/v1/nodes/{node_id}/settings`             | LocalNode でも自身の `node_id` で応答                                     |
| `PATCH /api/v1/settings/node`               | `PATCH /api/v1/nodes/{node_id}/settings/node`      | 同上                                                                      |
| `PATCH /api/v1/settings/agents`             | `PATCH /api/v1/nodes/{node_id}/settings/agents`    | 同上                                                                      |
| `GET /api/v1/secrets` (Node時)              | `GET /api/v1/nodes/{node_id}/secrets`              | サーバー秘密は `/api/v1/server/secrets` (または `/api/v1/secrets`) に分離 |
| `PUT /api/v1/secrets/{id}` (Node時)         | `PUT /api/v1/nodes/{node_id}/secrets/{id}`         | 同上                                                                      |
| `DELETE /api/v1/secrets/{id}` (Node時)      | `DELETE /api/v1/nodes/{node_id}/secrets/{id}`      | 同上                                                                      |
| `POST /api/v1/secrets/{id}/rotate` (Node時) | `POST /api/v1/nodes/{node_id}/secrets/{id}/rotate` | 同上                                                                      |

#### 2. `ClientApiBackend` の責務整理

Phase 1 の `NodeRpcRequest` を導入すると、ノードに対する操作は LocalNode（`DaemonState`）でも CentralServer（`ServerState`）でも **「対象 `node_id` に対して `NodeRpcRequest` を実行する」** という単一の抽象に畳み込める。

```rust
#[async_trait]
pub trait ClientApiBackend: Clone + Send + Sync + 'static {
    fn db(&self) -> &Db;
    fn auth_token(&self) -> String;
    fn connection_role(&self) -> ConnectionRole;

    /// 指定ノードに対する操作を実行する（LocalNode は自身で実行、CentralServer は NodeHub 経由で中継）
    async fn execute_node_rpc(
        &self,
        node_id: &NodeId,
        request: NodeRpcRequest,
        client: ClientInfo,
    ) -> Result<NodeRpcResponse, ApiError>;

    // 中央サーバー固有、またはロール別クエリのみ個別メソッドとして残す
    // (create_session, list_projects, list_agents, server_settings, server_secrets, node_tokens, provisioners, push_subscribe, kill_switch, pty_*)
}
```

これにより、`ClientApiBackend` のメソッド数を **約40個 → 約18個** に半減できる。

---

### 2.3 Phase 3: Client WS と REST のコマンド一本化・共通化

1. **権限承認・Elicitation 応答のペイロード統一**
   - `ClientWsMessage::RespondPermission` でも `RespondPermissionRequest` と同じフィールド（`always: bool` を含む）を受け取るか、または UI がすでに REST (`api.respondPermission` / `api.respondElicitation`) を使っているため、**冪等な状態変更リクエスト（Permission / Elicitation / Resume / Archive / Delete）は REST に一本化**し、Client WS の上りメッセージは `Subscribe`, `SendPrompt`, `ControlSession`, `Ping` に絞る。
2. **検索 API の HTTP メソッド整理**
   - `POST /api/v1/search`（`crates/fxg-server/src/api/mod.rs:1400-1422`）は現在 Query (`?q=...`) と JSON Body (`{"query": "..."}`) の両方を受け付ける冗長な実装になっているため、UI (`client.ts:389`) の実装に合わせて `GET /api/v1/search?q=...&limit=...`（または Body なしの Query 指定）に統一する。

---

### 2.4 Phase 4: CLI ⇔ Daemon IPC と `NodeRpc` の統合（任意・段階的）

- `crates/fxg-protocol/src/ipc.rs` の `IpcClientMessage` のうち、セッション操作・Worktree操作・プロジェクト操作・監査ログ記録を伴うものを `NodeRpcRequest` の呼び出しに委譲する。
- CLI 固有のローカル操作（`EnsureSession`, `SessionImport`, `GetLocalStatus`, `ProjectInfo`, `GitCredential`）のみを IPC 専用バリアントとして残すことで、`crates/fxg-node/src/daemon/ipc.rs` と `crates/fxg-node/src/daemon/api.rs` の実装重複を排除する。

---

## 3. 段階的チェックリスト

- [ ] **Phase 1: `NodeRpcRequest` / `NodeRpcResponse` の導入と `NodeHub` リレー統合**
  - [ ] `fxg-protocol::node_server` に `NodeRpcRequest` / `NodeRpcResponse` を定義
  - [ ] `NodeToServerMsg` の個別 `*Result` を `NodeRpcResult`（`Result<NodeRpcResponse, ErrorInfo>`）に統合
  - [ ] `fxg-server::hub::NodeHub` の `PendingResponse` と個別リレー関数を汎用 `rpc()` に集約
  - [ ] `fxg-node::sync::handle_server_message` の RPC ディスパッチを共通ハンドラに分離
- [ ] **Phase 2: Client REST API の重複エンドポイント統合と `ClientApiBackend` 縮小**
  - [ ] 設定・秘密のローカル専用パスを `/api/v1/nodes/{node_id}/...` と `/api/v1/server/...` に整理
  - [ ] `ClientApiBackend` のノード対象メソッドを `execute_node_rpc` 経由に集約
  - [ ] `ui/src/lib/api/client.ts` と Settings 画面の呼び出しメソッドを統一
- [ ] **Phase 3: Client WS / REST の重複整理**
  - [ ] `RespondPermission` / `RespondElicitation` の WS / REST 間のシグネチャ統一（または REST への一本化）
  - [ ] `/api/v1/search` のパラメータ受け渡し方式の統一
- [ ] **Phase 4: CLI IPC ハンドラと Node RPC 実行処理の共通化**
  - [ ] `fxg-node/src/daemon/ipc.rs` のセッション・プロジェクト・Worktree 操作を共通 RPC 実行関数に委譲
