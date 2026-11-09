# リファクタリング・プログラム（API統合・モジュール分割）

- **日付**: 2026-11-09
- **ステータス**: 進行中（B0 / B1 / A1 / A2 / A3 / A4 / B2 完了、次は B3 / B4）
- **統合元**: 同日に別々に作成した「API・プロトコル統合リファクタリング計画」と
  「コードベース全体リファクタリング・モジュール分割計画」を1本化した
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-acp`, `fxg-node`,
  `fxg-server`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**:
  `mise run fmt` / `mise run lint` / `mise run check` / `mise run types` /
  `cargo test --workspace` / `pnpm --dir ui run check` / `pnpm --dir ui run test`
- **関連ドキュメント**:
  - [`docs/README.md`](../README.md)
  - [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md)
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md)
  - [`docs/04-agent-drivers-and-windows.md`](../04-agent-drivers-and-windows.md)
  - [`docs/06-type-safety.md`](../06-type-safety.md)

---

## 0. この文書の位置づけ

- **実行順序の唯一の正**は [§2](#2-実行順序決定版) とする。個別の設計判断は
  Part A / Part B に置く。
- 進捗は [§6 チェックリスト](#6-チェックリスト) と
  [§7 決定ログ / 進捗メモ](#7-決定ログ--進捗メモ) だけを更新する。
- 完了した項目は削除せず `[x]` を付けて残す。設計の背景を後から追えるようにする。
- 旧2ファイルは本ドキュメントに統合済み。Git 履歴にのみ残す。

### 関連計画の状態

| 計画                                                                                                                     | 状態     | 本計画との関係                                                          |
| :----------------------------------------------------------------------------------------------------------------------- | :------- | :---------------------------------------------------------------------- |
| [`2026-11-09-settings-consolidation-and-project-identity.md`](2026-11-09-settings-consolidation-and-project-identity.md) | 実装済み | 秘密ストアの `fxg-db` 統合（B0-3）を完了済み                            |
| [`2026-11-08-type-safety-plan.md`](2026-11-08-type-safety-plan.md)                                                       | ほぼ完了 | open enum / `RawJson` / `ErrorInfo` / ID newtype の前提。Phase 5 のみ残 |
| [`2026-11-08-error-handling-and-recovery-plan.md`](2026-11-08-error-handling-and-recovery-plan.md)                       | 進行中   | A1 の `ErrorInfo` 透過はこの契約に乗る                                  |

---

## 1. 背景と現状

### 1.1 解消したい技術的負債

1. **2,000〜3,600行超の巨大単一ファイル化**:
   プロセス管理・プロトコル変換・DB操作・ビジネスロジック・単体テストが
   1ファイルに同居し、変更時の見通しが悪い。
2. **クレート間の重複実装（DRY 原則違反）**:
   秘密ストア（統合済み）、`SourcedValue` の解決、監査ログ記録などが
   `fxg-node` と `fxg-server` に重複している。
3. **通信境界ごとのボイラープレート**:
   1つの操作を追加するたびに、REST / Node RPC / CLI IPC の3境界・計7〜8箇所に
   型とハンドラを書く構造になっている。
4. **フロントエンドの状態・ロジック集約不足**:
   ページコンポーネントに API 呼び出し・モーダル状態・フォーム状態が直書きされ、
   データ取得のたびに非同期フロー全体へ修正が波及する。

### 1.2 現在の主要ファイル（2026-11-09 時点）

| ファイル                                                                                 |  行数 | 主な混在責務                                                                  |
| :--------------------------------------------------------------------------------------- | ----: | :---------------------------------------------------------------------------- |
| [`crates/fxg-acp/src/acp.rs`](../../crates/fxg-acp/src/acp.rs)                           | 3,603 | プロセス生成、JSON-RPC ハンドラ、起動/Resume/Fork、設定変換、テスト           |
| [`crates/fxg-node/src/session_manager.rs`](../../crates/fxg-node/src/session_manager.rs) | 3,356 | セッション管理、500ms フラッシュ、権限判定、Elicitation、履歴リプレイ、テスト |
| [`crates/fxg-cli/src/commands.rs`](../../crates/fxg-cli/src/commands.rs)                 | 3,095 | 全サブコマンドの `clap` 定義・実行、出力整形、テスト                          |
| [`crates/fxg-protocol/src/common.rs`](../../crates/fxg-protocol/src/common.rs)           | 2,061 | エラー、セッション、プロジェクト、Diff、ACP、監査の型                         |
| [`crates/fxg-server/src/api/mod.rs`](../../crates/fxg-server/src/api/mod.rs)             | 2,013 | `ApiError`、`ClientApiBackend`、Axum ルーター、全 REST ハンドラ、テスト       |
| [`crates/fxg-server/src/state.rs`](../../crates/fxg-server/src/state.rs)                 | 1,960 | `ServerState` 初期化、設定、トークン、セッション起動、Backend 実装、テスト    |
| [`crates/fxg-server/src/hub.rs`](../../crates/fxg-server/src/hub.rs)                     | 1,773 | Node WS、`PendingResponse`（13 variant）、個別リレー関数、同期適用            |
| [`crates/fxg-node/src/daemon/ipc.rs`](../../crates/fxg-node/src/daemon/ipc.rs)           | 1,660 | UDS / Named Pipe、フレーミング、IPC コマンド分岐、テスト                      |
| [`crates/fxg-node/src/sync.rs`](../../crates/fxg-node/src/sync.rs)                       | 1,363 | `ServerToNodeMsg` の巨大 match、個別 `*Result` の組み立て                     |
| [`ui/src/routes/projects/+page.svelte`](../../ui/src/routes/projects/+page.svelte)       |   833 | 一覧、Scan/Link、Worktree CRUD、Profile 編集                                  |
| [`ui/src/lib/sync/reducer.ts`](../../ui/src/lib/sync/reducer.ts)                         |   765 | タイムライン構築、チャンク結合、ツール状態、Permission/Elicitation            |
| [`ui/src/lib/api/client.ts`](../../ui/src/lib/api/client.ts)                             |   608 | 約60個の手書き REST ラッパー                                                  |

> `hub.rs` と `sync.rs` は A1 の完了で大幅に縮む見込み。分割対象に含めるかは
> 縮小後の行数で判断する（§4.6）。

### 1.3 現状の「1機能追加あたりの変更箇所」

1. **Client ⇔ Server/Node REST API**
   - `fxg-protocol/src/client_api.rs` に Request / Response を追加
   - `fxg-server/src/api/mod.rs` の `ClientApiBackend`（約40メソッド）に追加
   - 同ファイルの `client_router` に Axum ルートとハンドラを追加
   - `fxg-node/src/daemon/api.rs`（LocalNode）と `fxg-server/src/state.rs`
     （CentralServer）の両方に実装
   - `ui/src/lib/api/client.ts` に手書き fetch ラッパーを追加
2. **Server ⇔ Node 間リレー**
   - `ServerToNodeMsg` にリクエスト variant、`NodeToServerMsg` に `*Result`
   - `hub.rs` の `PendingResponse` と個別リレーメソッドを追加
   - `fxg-node/src/sync.rs` の `handle_server_message` に分岐を追加
3. **CLI ⇔ Daemon ローカル IPC**
   - `fxg-protocol/src/ipc.rs` の `IpcClientMessage` / `IpcResult` に追加
   - `fxg-node/src/daemon/ipc.rs` と `fxg-cli/src/client.rs` に実装を追加

### 1.4 具体的な問題点

| #   | 問題                                                      | 該当箇所                                                     | 影響                                                                                                                                                      |
| :-- | :-------------------------------------------------------- | :----------------------------------------------------------- | :-------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | **NodeHub リレーの個別定義とエラー情報の欠落**            | `fxg-protocol/src/node_server.rs`<br>`fxg-server/src/hub.rs` | RPC ごとに `*Result` と `PendingResponse` が増え、`GitDiffResult` 等は `error: Option<String>` しか持たないため `ErrorCode` / `RecoveryAction` が潰れる。 |
| 2   | **LocalNode 用と Hub Proxy 用の REST エンドポイント重複** | `fxg-server/src/api/mod.rs`<br>`ui/src/lib/api/client.ts`    | 設定・秘密・エージェントに2系統のパスがあり、`ClientApiBackend` に `InvalidState` を返すスタブが大量発生している。                                        |
| 3   | **REST と Client WS のコマンド重複とシグネチャ乖離**      | `fxg-protocol/src/client_api.rs`                             | `RespondPermission` が REST と WS の両方にあり、REST 側にのみ `always: bool` がある。                                                                     |
| 4   | **`ClientApiBackend` トレイトの肥大化**                   | `fxg-server/src/api/mod.rs`                                  | サーバー専用機能とノード操作が混在し、約40メソッドに膨張している。                                                                                        |
| 5   | **CLI IPC と Client API / Node RPC の型分離**             | `fxg-protocol/src/ipc.rs`<br>`fxg-node/src/daemon/ipc.rs`    | 同じ `DaemonState` への操作に専用の Request / Result / ディスパッチャを別途維持している。                                                                 |

---

## 2. 実行順序（決定版）

### 2.1 原則

- **統合で消えるコードを先に分割しない**。重なるファイルは `A`（API統合）→ `B`
  （分割）の順に進める。
- `A` と重ならないファイル（`common.rs`, `acp.rs`, `session_manager.rs`, UI）は
  先行・並行してよい。
- **wire format を変える A1 は1ブランチにまとめる**。途中の master に
  半端なプロトコルを残さない。
- **分割 PR は移動のみ**。公開 API・wire format・テスト件数を変えない。
- 1 PR = 1 サブ項目。ただし A1 は例外的に `refactor/node-rpc` ブランチへ
  4コミットを積み、まとめてマージする。

### 2.2 重なるファイルの扱い

| 対象ファイル                                       | 関係する計画 | 推奨順序                                              |
| :------------------------------------------------- | :----------- | :---------------------------------------------------- |
| `hub.rs` / `daemon/ipc.rs` / `client.ts`           | A のみ       | A で縮小 → 分割は不要になる見込み。縮小後に行数で判断 |
| `api/mod.rs` / `state.rs`                          | A + B        | A を先に実施し、新しい構造に合わせて B で分割         |
| `commands.rs`                                      | A + B        | 先に純粋分割してよい（`clap` 定義は安定している）     |
| `acp.rs` / `session_manager.rs` / `common.rs` / UI | B のみ       | A と独立。B1 完了後ならいつ着手してもよい             |

### 2.3 実行順序

| 順序 | ID     | 作業                                                   | 規模 | 主な対象                               |
| ---: | :----- | :----------------------------------------------------- | :--- | :------------------------------------- |
|    0 | P0     | 計画整理・ベースライン確認                             | S    | docs, `mise run check`                 |
|    1 | B0     | 小さな DRY 回収（`SourcedValue`, 監査ログ）            | S    | `fxg-protocol`, `fxg-db`, node, server |
|    2 | B1     | `common.rs` 分割とテスト fixture                       | M    | `fxg-protocol`                         |
|    3 | A1-pre | プロトコル互換性ゲート                                 | S    | `node_server`, `hub`                   |
|    4 | A1-a   | `NodeRpc` 導入・読み取り系の移行                       | L    | protocol / node / server               |
|    5 | A1-b   | 変更系の移行と旧リレー削除                             | L    | protocol / node / server               |
|    6 | A1-c   | 旧 `*Result` 削除・ドキュメント更新                    | M    | protocol / node / server / docs        |
|    7 | A2     | REST 統合・`ClientApiBackend` 縮小・`client.ts` 汎用化 | M    | `fxg-server`, `ui`                     |
|    8 | A3     | Client WS / REST の重複整理                            | S    | protocol / server / ui                 |
|    9 | B4a    | `commands.rs` の純粋分割                               | M    | `fxg-cli`                              |
|   10 | A4     | CLI を `NodeRpc` 体系へ統合                            | L    | protocol / node / cli                  |
|   11 | B2     | `state.rs` / `api/mod.rs` の分割                       | M    | `fxg-server`                           |
|   12 | B3     | `acp.rs` / `session_manager.rs` の分割                 | L    | `fxg-acp`, `fxg-node`                  |
|   13 | B4b    | UI ページ・reducer の分割                              | M    | `ui`                                   |
|   14 | B5     | UI データフローの宣言化                                | M    | `ui`                                   |
|   15 | B6     | DB Row と Protocol 型のマッピング整理                  | M    | `fxg-db`, `fxg-protocol`               |

> B3 は A とファイルが重ならないため、B1 完了後ならどこかで並行してよい
> （別 worktree を使う場合も、同じファイルを触る作業とは並行させない）。

### 2.4 PR 運用

- ブランチ名は `refactor/<id>-<slug>`（例: `refactor/a1-node-rpc`）。
- 各 PR で `mise run fmt` / `mise run lint` / `cargo test --workspace` /
  `mise run types` / `pnpm --dir ui run check` を通す。
- 計画のチェックリストと進捗メモは同じ PR で更新する。
- 分割 PR は「移動のみ」であることをレビューで確認する。diff にロジック変更が
  混ざっていたら分割し直す。

---

## 3. Part A: API・プロトコル統合

### 3.1 A1-pre: プロトコル互換性ゲート

`NodeHello.version` は現在 DB に保存されるだけで、互換性検証には使っていない。
サーバーだけを更新して古いデーモンが接続すると、未知の variant で静かに切断される。

- `fxg-protocol` に `PROTOCOL_VERSION: u32` を追加する。
- `NodeHello` に `protocol_version: u32` を追加する。旧ノードはフィールドを
  送らないため `#[serde(default)]` で 0 として受理し、サーバーが最小バージョン
  未満を明確なログとともに拒否する。
- 「同一リリースの server / daemon を同時に更新する」ことを契約とし、
  後方互換の分岐は追加しない（AGENTS.md の方針）。
- 逆方向（新しい daemon + 古い server）はサポート外であることを
  `docs/03-protocol-and-api.md` に明記する。
- テスト: `0` は拒否、現行値は通過、フィールド欠落は `0` としてパース。

### 3.2 A1-a / A1-b: `NodeRpc` によるリレー統合

#### スコープ

`StartSession` / `SendPrompt` / `RespondPermission` / `RespondElicitation` /
`ControlSession` は**今回は移行しない**。これらはすでに `CommandResult` という
共通の結果型を使っており、重複の本体は以下12系統の `*Result` と
`PendingResponse` / 個別リレーメソッドである。

- 読み取り系（A1-a）: `GetGitDiff`, `BrowseFs`, `ProjectScan`, `ListAgents`,
  `GetNodeSettings`, `GetSecrets`
- 変更系（A1-b）: `ManageWorktree`, `ProjectLink`, `ManageAgent`,
  `UpdateNodeSettings`, `UpdateAgentSettings`, `UpsertSecret`, `DeleteSecret`,
  `ResumeSession`, `ArchiveSession`, `DeleteSession`

#### プロトコル構造

```rust
pub enum ServerToNodeMsg {
    // 同期・ストリーム・ライフサイクル制御は変更しない
    EventBatchAck { /* ... */ },
    ResyncRequest { /* ... */ },
    PtySpawn { /* ... */ },
    // ...

    /// 汎用 Node RPC リクエスト
    NodeRpc {
        request_id: CommandId,
        request: NodeRpcRequest,
    },
}

pub enum NodeToServerMsg {
    // 同期・ストリームは変更しない
    NodeHello { /* ... */ },
    EventBatchPush { /* ... */ },
    // ...

    /// 汎用 Node RPC レスポンス
    NodeRpcResult {
        request_id: CommandId,
        outcome: NodeRpcOutcome,
    },
}

/// `Result<_, ErrorInfo>` をそのまま wire に載せず、明示的な enum にする。
pub enum NodeRpcOutcome {
    Ok(NodeRpcResponse),
    Err(ErrorInfo),
}
```

- `NodeRpcRequest` は `client_api` の Request 型を再利用する。同じ要求を
  第3の型として再定義しない。
- `NodeRpcResponse` は既存の `client_api` DTO を包む。
- `NodeRpcRequest::timeout(&self) -> Duration` で操作別タイムアウトを一元管理する。
  - 通常: 30秒
  - FS ブラウズ / プロジェクトスキャン / セッション再開: 120秒
  - エージェント導入: 600秒

#### ノード側

- `fxg-node/src/rpc.rs` を追加し、`NodeRpcRequest::execute(&DaemonState)` を置く。
- `sync.rs` は `ServerToNodeMsg::NodeRpc` を match して `execute` に渡すだけにする。
  巨大 match に個別処理を書き足さない。
- エラーは `NodeError::info()` で `ErrorInfo` に変換し、`NodeRpcOutcome::Err` に載せる。

#### Hub 側

- `hub.rpc(node_id, request) -> Result<NodeRpcResponse, ErrorInfo>` を追加する。
  `PendingResponse` は `Rpc(NodeRpcOutcome)` と既存の制御系だけに縮小する。
- `ErrorInfo` を `ApiError` に再ラップしない。`ErrorCode` → HTTP status の変換は
  `api/error.rs` の1箇所だけに置く。
- 移行済みの個別リレーメソッド（`git_diff`, `worktree_op`, `resume_session`,
  `archive_session`, `delete_session`, `browse_fs`, `scan_projects`,
  `link_project`, `agents`, `manage_agent`, `node_settings`, `secret_op`）を削除する。

#### 期待効果

- `PendingResponse` の13 variant と約350行の個別リレーが消える。
- すべてのノード操作で `ErrorCode` / `RecoveryAction` / `DataSafety` が欠落せず
  クライアントへ届く。
- `hub.rs` は1,000行前後まで縮み、分割不要になる見込み。

### 3.3 A1.5（保留）: セッションコマンドの `NodeRpc` 統合

`StartSession` 等を `NodeRpcRequest` に畳む作業。すでに `CommandResult` を
共有しているため効果は限定的で、ACP の起動経路という高リスク領域を触る。
**A1 完了後に dual dispatch の痛みが残っている場合だけ着手する。**

### 3.4 A2: Client REST API の統合

#### 前提: LocalNode の自己 `node_id`

ノード所有リソースを `/api/v1/nodes/{node_id}/...` に一本化するには、UI が
接続先の `node_id` を知る必要がある。`MetaResponse` は認証前に返るため、
認証後の `SystemInfoResponse` に `node_id: Option<NodeId>` を追加した
（LocalNode は `Some`、中央サーバーは `None`）。UI は `system/info` の値を
ノードスコープのパスに使う。

#### 廃止するローカル専用エンドポイント

| 廃止するパス                            | 統一後のパス                                    |
| :-------------------------------------- | :---------------------------------------------- |
| `GET /api/v1/settings/node`             | `GET /api/v1/nodes/{node_id}/settings`          |
| `PATCH /api/v1/settings/node`           | `PATCH /api/v1/nodes/{node_id}/settings/node`   |
| `PATCH /api/v1/settings/agents`         | `PATCH /api/v1/nodes/{node_id}/settings/agents` |
| `GET /api/v1/secrets`（Node時）         | `GET /api/v1/nodes/{node_id}/secrets`           |
| `PUT /api/v1/secrets/{id}`（Node時）    | `PUT /api/v1/nodes/{node_id}/secrets/{id}`      |
| `DELETE /api/v1/secrets/{id}`（Node時） | `DELETE /api/v1/nodes/{node_id}/secrets/{id}`   |

- サーバー所有の秘密は `/api/v1/server/secrets` に分離する。ロールによって
  同じパスの意味が変わる現状をやめる。
- `rotate` は `14dd6cb` で廃止済みのため、移行対象に含めない。

#### `ClientApiBackend` の縮小

```rust
#[async_trait]
pub trait ClientApiBackend: Clone + Send + Sync + 'static {
    fn db(&self) -> &Db;
    fn auth_token(&self) -> String;
    fn connection_role(&self) -> ConnectionRole;

    /// 指定ノードに対する操作（LocalNode は自身で実行、Hub は中継）
    async fn execute_node_rpc(
        &self,
        node_id: &NodeId,
        request: NodeRpcRequest,
        client: Option<ClientInfo>,
    ) -> Result<NodeRpcResponse, ApiError>;

    // サーバー固有・ロール別クエリのみ個別メソッドとして残す
    // (create_session, list_projects, server_settings, server_secrets,
    //  node_tokens, provisioners, push_subscribe, kill_switch, pty_*)
}
```

`client` は LocalNode が監査ログの source に使う（Hub は無視し、ノード側が
relay 元を記録する）。A2 では設定・秘密・`browse_fs` のノード対象メソッド
（13個）を削除してトレイトを 57 → 49 メソッドに削減した。完全な18メソッド化は
ハンドラ側の監査・集約ロジックの移設が必要なため、A3/A4 の結果を見て判断する。

#### `ui/src/lib/api/client.ts`

- ローカル専用とハブ経由で重複していた設定・秘密のメソッドを削除し、
  `/nodes/{node_id}/...` と `/server/...` の1系統に統一した。
- ノード操作はノードIDを明示する薄いラッパー（`nodeSettings(nodeId)` 等）に
  集約した。UI の呼び出し側でローカル/リモートの分岐を書かない。
- 汎用ディスパッチャ（`rpc(nodeId, request)`）への完全移行は、REST を型付き
  RPC エンドポイントへ置き換える場合の課題として A4 以降に残す。
- `client_api` の型変更後に `mise run types` を実行する。

### 3.5 A3: Client WS / REST の重複整理

- 上りの Client WS は `Subscribe`, `SendPrompt`, `ControlSession`, `Ping` に絞る。
- Permission / Elicitation / Resume / Archive / Delete は REST に一本化する
  （UI はすでに REST を使用）。
- `POST /api/v1/search` は Query と JSON Body の両対応をやめ、
  `GET /api/v1/search?q=...&limit=...` に統一する。

### 3.6 A4: CLI をローカル RPC クライアントへ統合

- Named Pipe / UDS はトランスポートとして維持する。
- 送受信するメッセージを `NodeRpcRequest` / `NodeRpcResponse` とローカル専用
  クエリ（`GetLocalStatus` 等）に統一する。
- `IpcClientMessage` / `IpcResult` を廃止し、`daemon/ipc.rs` の専用
  ディスパッチャを共通 RPC ハンドラへ寄せる。
- ローカル IPC にも `PROTOCOL_VERSION` 検査を入れる（daemon と CLI の
  バージョンずれを明確に落とす）。
- 先に B4a で `commands.rs` を分割しておき、差分を小さくする。

---

## 4. Part B: モジュール分割と重複排除

### 4.1 B0: 小さな DRY 回収

#### B0-1: `SourcedValue` の解決ロジック

- 現状: `crates/fxg-node/src/daemon/ops.rs` と
  `crates/fxg-server/src/state.rs` に、env → `config.toml` → CLI → default の
  優先順位を判定するヘルパーが重複している。
- 改善: `fxg-protocol::settings`（`SourcedValue` の定義場所）に候補チェーンを
  持つビルダーを置き、両者から呼び出す。
- 優先順位のテストを1箇所に集約する。

#### B0-2: 監査ログ記録

- `fxg-node` は `DaemonState::record_audit`、`fxg-server` は `state.rs` 内で
  個別に `AuditLogRecord::new(...)` + `append_audit_log` を組み立てている。
- `Db::record_audit(...)` を用意し、失敗時の `tracing::warn!` まで含めて統一する。

#### B0-3: 秘密ストア（完了）

- `crates/fxg-node/src/secrets.rs` と `crates/fxg-server/src/secrets.rs` を
  `fxg-db::secrets` に統合済み（`14dd6cb`）。
- `SecretKind` とランダム `rotate` は廃止し、キー名で参照する汎用 Key-Value
  ストアに再定義済み。本計画では追加作業なし。

### 4.2 B1: `fxg-protocol` の分割とテスト fixture

#### `common.rs` の分割

`common/mod.rs`（`pub use` のみ）＋以下へ分割する。呼び出し側は
`common::*` のままで壊さない。

- `common/mod.rs`: `RawJson`, `EnumParseError`, モジュール宣言と `pub use`
- `common/error.rs`: `ErrorCode`, `ErrorInfo`, `RecoveryAction`, `DataSafety`,
  `WarningInfo`, `CommandResult`, `CommandStatus`
- `common/session.rs`: `SessionSummary`, `SessionStatus`, `PromptInput`,
  `SessionControlAction`, usage / compaction / stream delta
- `common/project.rs`: `ProjectSummary`, `NodeSummary`, `NodeLifecycleStatus`,
  `WorkspaceDiffResponse`, worktree / diff 型
- `common/acp.rs`: `Permission*`, `Elicitation*`, `ConfigOption*`, `ToolCall*`,
  `ModeInfo`, `CommandInfo`, `PlanEntry`
- `common/agent.rs`: `AgentAction`, `AgentAuthMethod*`
- `common/audit.rs`: `EventType`, `SearchHit`, `AuditAction`, `AuditLogEntry`

#### テスト fixture

構造体にフィールドを1つ足すたび、各クレートのテストにある構造体リテラルが
一斉に壊れる問題を防ぐ。

- `testing` feature を追加し、`SessionSummary::dummy()` /
  `ProjectSummary::dummy()` / `NodeSummary::dummy()` などを用意する。
- `Default` は実装しない。型安全プランの「ID に `Default` を付けない」と
  矛盾するため、テスト専用の明示的なビルダーにする。
- 下流クレートは `[dev-dependencies]` で `features = ["testing"]` を有効にする。

### 4.3 B2: `fxg-server` の分割（A2 後）

- `state/mod.rs`: `ServerState` 本体
- `state/settings.rs`: `ServerSettings` / `ProjectProfile` / Secrets
- `state/sessions.rs`: セッション起動・Provisioner・Fork 検証
- `state/api_impl.rs`: `ClientApiBackend for ServerState`
- `api/mod.rs`: ルーターと共通型
- `api/error.rs`: `ApiError` と `ErrorCode` → HTTP status
- `api/backend.rs`: `ClientApiBackend`
- `api/handlers/`: A2 後のサイズを見てドメイン別に分割する。小さければ
  1ファイルにまとめたままでよい。

> Rust は同一型に対するトレイト実装を1つしか許さないため、`sessions.rs` は
> トレイトメソッド本体を `*_impl` の固有メソッドとして提供し、`api_impl.rs` の
> 1つの `impl ClientApiBackend for ServerState` がそれらへ委譲する。

### 4.4 B3: `fxg-acp` と `fxg-node` の分割

- `acp/mod.rs`: `AcpDriver` の公開 API
- `acp/client.rs`: ACP Client トレイト実装・コールバック
- `acp/process.rs`: 子プロセス・stderr 収集・認証検知
- `acp/mapping.rs`: ToolCall / Plan / Config / Elicitation の型変換
- `acp/tests.rs`

- `session_manager/mod.rs`
- `session_manager/lifecycle.rs`: start / resume / fork / archive / delete
- `session_manager/stream.rs`: チャンク結合・500ms フラッシュ・永続化
- `session_manager/approval.rs`: `PermissionPolicy` / Elicitation 応答
- `session_manager/replay.rs`: 履歴リプレイ構築

子モジュールは親モジュールの private フィールドを参照できるため、
`impl` ブロックの分割だけで済む。テストは移動先のモジュールへ移す。

### 4.5 B4: CLI と UI の分割

#### B4a: CLI（A4 の前）

- `commands/mod.rs`: `Cli` 構造体・エントリ
- `commands/run.rs`: `fxg run` とストリーム表示
- `commands/session.rs`: `ps`, `session *`, `inbox`
- `commands/project.rs`: `project *`, `worktree *`
- `commands/admin.rs`: `agents`, `nodes`, `provisioners`, `auth`, `doctor`

#### B4b: UI

- `ui/src/routes/projects/+page.svelte` から
  `ProjectCard.svelte`, `WorktreeManagerDialog.svelte`,
  `ProjectProfileDialog.svelte`, `ProjectLinkForms.svelte` を
  `ui/src/lib/components/projects/` へ切り出す。
- `ui/src/lib/sync/reducer.ts` はアイテム種別ごとのハンドラ関数に整理する。
  純粋関数のまま保ち、既存テストを移動する。

### 4.6 分割不要になったファイル

- `hub.rs`: A1 で約350行のリレーと `PendingResponse` が消えるため、分割せず
  1,200行以下に収まるか確認する。
- `daemon/ipc.rs`: A4 で `IpcClientMessage` が消え、ディスパッチが共通化される
  ため、分割せず収まるか確認する。
- `sync.rs`: A1 で `NodeRpc` 1分岐に縮むため、分割対象にしない。

### 4.7 B5: UI データフローの宣言化

- 現状: 各ページで `loading` / `error` / `busy` と手動再取得
  （`sync.refreshProjects()`）を個別実装している。
- 改善: Svelte 5 の Runes に合う軽量な Resource ヘルパーを用意し、
  「取得」「エラー」「無効化」を宣言的に書けるようにする。
- まず `projects` ページ1枚で試作し、効果を確認してから他ページへ広げる。
- TanStack Query / Svelte Query の導入は、試作で不足が確認できた場合のみ検討する。
  依存追加を目的にしない。

### 4.8 B6: DB Row と Protocol 型のマッピング整理

- 現状: `SessionRow` と `SessionSummary` のようにほぼ同一定義を二重に持ち、
  `into_summary()` を手書きしているため、カラム追加が
  migration → Row → マッピング → Summary の4重変更になる。
- 方針:
  - **`fxg-protocol` に `sqlx::FromRow` を持ち込まない**。protocol と DB の
    結合を強め、レイヤ境界を壊すため。
  - 1対1の読み取りモデルは、`fxg-db` のクエリが protocol DTO を直接返す。
    DB 固有カラムがある場合だけ Row 構造体を残す。
  - 書き込み・更新は明示的なマッピングを維持する。
  - どうしても必要な箇所はマクロでマッピングを生成し、フィールド追加時の
    修正漏れをコンパイルエラーにする。
- `metadata: Option<RawJson>` のような拡張カラムは、本当に開いている
  メタデータにだけ使う。中核フィールドの逃げ道にしない。

---

## 5. 変更波及を抑える仕組み

| 仕組み                                     | 目的                                                   |
| :----------------------------------------- | :----------------------------------------------------- |
| テスト fixture（B1）                       | 構造体拡張時のテスト修正コストをゼロに近づける         |
| UI Resource ヘルパー（B5）                 | API 変更時の非同期フロー全体への波及を抑える           |
| protocol DTO を直接返す DB クエリ（B6）    | カラム追加時の4重変更を減らす                          |
| `NodeRpc` の共通エンベロープ（A1）         | 操作追加時の variant / relay / dispatch の増殖を止める |
| `ClientApiBackend::execute_node_rpc`（A2） | LocalNode と Hub の実装分岐をなくす                    |
| 汎用 `client.ts` ヘルパー（A2）            | フロントエンドの手書きラッパーをなくす                 |

---

## 6. チェックリスト

### P0: 計画整理・ベースライン

- [x] 旧2計画を本ドキュメントへ統合
- [x] `mise run check` で緑を確認
- [x] 旧計画の参照行・廃止済み項目（`rotate` / `SecretKind`）を除去
- [x] プロトコル互換性の方針を決定（同一リリース同時更新）

### B0: 小さな DRY

- [x] 秘密ストアを `fxg-db::secrets` に統合（`14dd6cb`）
- [x] `SourcedValue` の解決ビルダーを `fxg-protocol::settings` に集約
- [x] 監査ログ記録を `Db::record_audit` に統一

### B1: `fxg-protocol`

- [x] `common.rs` を `error` / `session` / `project` / `acp` / `agent` / `audit` に分割
- [x] `testing` feature と `dummy()` ビルダーを追加

### A1: NodeRpc 統合

- [x] A1-pre: `PROTOCOL_VERSION` と `NodeHello.protocol_version`、サーバー側検査
- [x] A1-a: `NodeRpcRequest` / `NodeRpcResponse` / `NodeRpcOutcome` を追加
- [x] A1-a: 読み取り系（git diff / browse fs / scan / agents / settings 取得 / secrets 一覧）を移行
- [x] A1-a: `hub.rpc()` と `NodeRpcRequest::timeout()` を追加
- [x] A1-b: 変更系（worktree / link / agent 管理 / settings 更新 / secrets 更新 / resume / archive / delete）を移行
- [x] A1-b: 個別リレーメソッドと `PendingResponse` の個別 variant を削除
- [x] A1-c: 旧 `*Result` と `error: Option<String>` を削除
- [x] A1-c: `sync.rs` の match を `NodeRpc` 1分岐に縮小
- [x] A1-c: `docs/03-protocol-and-api.md` を更新
- [x] A1-c: `hub.rs` の行数を測り、分割要否を判断（1,208行のため分割不要）
- [ ] A1.5（保留）: セッションコマンド variant の統合

### A2: REST 統合

- [x] `SystemInfoResponse` に `node_id` を追加（認証後の `system/info`）
- [x] 設定・秘密のローカル専用パスを `/nodes/{node_id}/...` と `/server/...` に整理
- [x] `execute_node_rpc` を追加し、設定・秘密・`browse_fs` のノード対象メソッドを削除（57 → 49）
- [x] `ui/src/lib/api/client.ts` のローカル/リモート重複メソッドを削除
- [x] `mise run types` で bindings を再生成
- [ ] 汎用ディスパッチャ化（REST の型付き RPC 化と合わせて A4 以降で判断）

### A3: WS / REST

- [x] Client WS の上りを `Subscribe` / `SendPrompt` / `ControlSession` / `Ping` に絞る
- [x] Permission / Elicitation / Resume / Archive / Delete を REST に一本化
- [x] `/api/v1/search` を GET のクエリ指定に統一

### A4: CLI IPC

- [x] `IpcClientMessage` / `IpcResult` を `IpcRequest` / `LocalRequest` / `LocalResult` と `NodeRpc` 体系へ統合
- [x] `daemon/ipc.rs` の Node 操作を `rpc::execute` へ委譲（ローカル専用操作は `dispatch_local` に分離）
- [x] ローカル IPC のバージョン検査を追加（`IPC_PROTOCOL_VERSION`、不一致は `VERSION_MISMATCH`）
- [ ] `commands.rs` の純粋分割（B4a。A4 では commands.rs を変更しなかったため独立して実施可能）

### B2: `fxg-server`

- [x] `state.rs` を `settings` / `sessions` / `api_impl` に分割
- [x] `api/mod.rs` を `error` / `backend` / `handlers` に分割
- [x] `api/handlers/` を `system` / `projects` / `sessions` / `agents` / `admin` / `settings` / `stream` に分割

### B3: `fxg-acp` / `fxg-node`

- [ ] `acp.rs` を `client` / `process` / `mapping` / `tests` に分割
- [ ] `session_manager.rs` を `lifecycle` / `stream` / `approval` / `replay` に分割

### B4: CLI / UI

- [ ] `commands.rs` を `run` / `session` / `project` / `admin` に分割
- [ ] `projects/+page.svelte` からサブコンポーネントを分離
- [ ] `sync/reducer.ts` をアイテム種別ごとのハンドラに整理

### B5: UI データフロー

- [ ] `projects` ページで Resource ヘルパーを試作
- [ ] 効果を確認して他ページへ展開
- [ ] TanStack Query 導入の要否を判断

### B6: DB マッピング

- [ ] 1対1の読み取りモデルを protocol DTO 直接返却へ移行
- [ ] 残る Row 構造体のマッピングを整理
- [ ] `metadata` 拡張カラムの適用範囲を文書化

---

## 7. 決定ログ / 進捗メモ

- **2026-11-09**: 旧2計画を統合し、実行順序を一本化した。重なるファイルは
  A（API統合）→ B（分割）、重ならないファイルは先行可とした。
- **2026-11-09**: A1 のスコープを「12系統の `*Result` と個別リレーの統合」に
  限定し、`StartSession` 等のセッションコマンドは A1.5 として保留した。
  すでに `CommandResult` を共有しており、ACP 起動経路を触るリスクに見合わない。
- **2026-11-09**: `Result<_, ErrorInfo>` を wire に直接載せず、
  `NodeRpcOutcome` を定義する。
- **2026-11-09**: 後方互換の分岐は追加しない。`PROTOCOL_VERSION` で
  古いデーモンを明示的に拒否し、同時更新を契約とする。
- **2026-11-09**: `rotate` と `SecretKind` は `14dd6cb` で廃止済みのため、
  計画から削除した。
- **2026-11-09**: テスト fixture は `Default` ではなく `testing` feature の
  `dummy()` とする（型安全プランと整合）。
- **2026-11-09**: `fxg-protocol` に `sqlx::FromRow` を持ち込まず、1対1の
  読み取りは DB クエリが DTO を直接返す方式を第一候補とする。
- **2026-11-09**: UI のデータ取得は軽量な Resource ヘルパーを試作してから
  判断する。TanStack Query は現時点では導入しない。
- **2026-11-09 (B0 実装)**: `fxg-protocol::settings` に `sourced_string` /
  `sourced_option` を追加し、環境変数を注入可能にして単体テストを追加した。
  `fxg-node` / `fxg-server` の重複ヘルパーを削除し、`source_detail` に既定値
  や設定キーを CLI フラグとして表示していた誤りも修正した。
- **2026-11-09 (B0 実装)**: `AuditLogRecord` にビルダーメソッドを追加し、
  `Db::record_audit` に「失敗時は warn のみ」の共通処理を集約した。
  `DaemonState::record_audit` と `ServerState::record_audit` は記録の組み立て
  だけを行う薄いラッパーになった。
- **2026-11-09 (B1 実装)**: `fxg-protocol/src/common.rs` を
  `common/{error,session,project,acp,agent,audit}.rs` に分割した。
  `common/mod.rs` の `pub use` により `crate::common::*` のパスは変えていない。
  テストは `common/error.rs` の `tests` モジュールへ移した。
- **2026-11-09 (B1 実装)**: `testing` feature と
  `SessionSummary::dummy()` / `ProjectSummary::dummy()` /
  `NodeSummary::dummy()` を追加した。`Default` は実装せず、ID に空値を
  入れない方針を維持している。
- **2026-11-09 (A1 実装)**: `NodeHello.protocol_version` と
  `PROTOCOL_VERSION` を追加し、サーバーが古いノードを `NodeHello` 時点で
  拒否するようにした。フィールド欠落は `0` としてデコードされる。
- **2026-11-09 (A1 実装)**: `NodeRpcRequest` / `NodeRpcResponse` /
  `NodeRpcOutcome` と `NodeRpc` / `NodeRpcResult` を追加し、12系統の
  `*Result` と `error: Option<String>` を削除した。`PendingResponse` は
  `Command` / `Rpc` / `Failed` の3 variant になった。
- **2026-11-09 (A1 実装)**: ノード側のディスパッチを
  `crates/fxg-node/src/rpc.rs` に分離した。`sync.rs` は `NodeRpc` を
  `rpc::execute` に渡して結果を返すだけになり、1,363行 → 833行に縮小した。
  `hub.rs` は 1,773行 → 1,208行となり、分割不要と判断した。
- **2026-11-09 (A1 実装)**: `ServerToNodeMsg` / `NodeToServerMsg` の
  セッションコマンド（`StartSession` 等）は A1.5 として保留したまま
  `CommandResult` を共有している。
- **2026-11-09 (A2 実装)**: `SystemInfoResponse.node_id` を追加し、UI が
  ローカルノードでも `/api/v1/nodes/{node_id}/...` を使えるようにした。
  設定・秘密のローカル専用パス（`/settings/node`, `/settings/agents`,
  `/secrets`）を削除し、サーバー秘密を `/api/v1/server/secrets` に分離した。
- **2026-11-09 (A2 実装)**: `ClientApiBackend::execute_node_rpc` を追加し、
  LocalNode はインプロセスで `rpc::execute` を呼び、Hub は `hub.rpc` に中継する
  形に統一した。設定・秘密・`browse_fs` のノード対象メソッドを削除し、
  トレイトは 57 → 49 メソッドになった。
- **2026-11-09 (A2 実装)**: `rpc::execute` に `AuditSource` を渡すようにし、
  ローカル REST 経由の操作も実クライアント情報で監査ログを記録できるように
  した（従来は relay 元の `remote` 固定だった）。
- **2026-11-09 (A2 実装)**: `ui/src/lib/api/client.ts` の設定・秘密ラッパーから
  ローカル/リモートの重複を削除した。汎用 RPC ディスパッチャ化は、REST を
  型付き RPC エンドポイントへ置き換える判断（A4 以降）と合わせて行う。
- **2026-11-09 (A3 実装)**: `ClientWsMessage` / `ClientCommand` から
  `RespondPermission` / `RespondElicitation` を削除し、上りを `Subscribe` /
  `SendPrompt` / `ControlSession` / `Ping` に限定した。UI の回答操作は REST
  （`already_resolved` を返す冪等 API）へ移行した。
- **2026-11-09 (A3 実装)**: `/api/v1/search` を `GET ?q=...&limit=...` に統一し、
  JSON body との二重受け付けを廃止した。
- **2026-11-09 (A4 実装)**: ローカル IPC を `IpcRequest`（`protocol_version` +
  `request_id`）で包み、`NodeRpcRequest` と CLI 専用の `LocalRequest` を
  `IpcRequestKind` で分岐する形にした。`IpcClientMessage` / `IpcResult` は廃止。
- **2026-11-09 (A4 実装)**: `ProjectLink` / `ProjectScan` / `SessionResume` /
  `SessionArchive` / `SessionDelete` は `NodeRpcRequest` をローカル実行する
  経路に移行し、`LocalResult` から重複した結果型を削除した。
- **2026-11-09 (A4 実装)**: `IPC_PROTOCOL_VERSION` を追加し、不一致時は
  `VERSION_MISMATCH` を返して接続を閉じる。CLI とデーモンのバージョンずれを
  明確に検出できる。
- **2026-11-09 (A4 補足)**: `SendPrompt` / `RespondPermission` /
  `RespondElicitation` / `ControlSession` は A1.5 が保留のため `LocalRequest`
  に残した。Worktree 操作は `cwd` からプロジェクトを解決するためローカルのまま。
- **2026-11-09 (B2 実装)**: `state.rs` を `state/{mod,settings,sessions,api_impl}.rs`
  に分割した。`mod.rs` は `ServerState` の構築・アクセサ・監査、`settings.rs` は
  サーバー設定・秘密・プロジェクトプロファイル、`sessions.rs` はセッション
  ライフサイクルと解決ヘルパー、`api_impl.rs` は `ClientApiBackend` 実装を持つ。
- **2026-11-09 (B2 実装)**: Rust では同一型に同じトレイトを複数回実装できない
  ため、`sessions.rs` のセッションメソッドは `*_impl` の固有メソッドとし、
  `api_impl.rs` のトレイト実装が委譲する形にした。公開 API・wire format・
  挙動は変えていない。
- **2026-11-09 (B2 実装)**: `api/mod.rs` を `api/{mod,error,backend}.rs` と
  `api/handlers/{system,projects,sessions,agents,admin,settings,stream}.rs` に
  分割した。`mod.rs` はルーター・`ClientApiState`・WS 共通ヘルパー、
  `error.rs` は `ApiError` と `ErrorCode` → HTTP status 変換、`backend.rs` は
  `ClientApiBackend` と関連型を持つ。ルート定義は分割前と同一。
