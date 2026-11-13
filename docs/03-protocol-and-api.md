# 03. プロトコル・API仕様 (`fxg-protocol`)

`crates/fxg-protocol` は、Node ⇔ Hub（WebSocket / Stdio）と
Client ⇔ Hub / Local Node（HTTP / WebSocket）で共有する型を定義します。
型・enum・フィールドの正はソースと生成済み bindings です。

- プロトコル型: [`crates/fxg-protocol/src/`](../crates/fxg-protocol/src/)
- UI 用の生成型: `ui/src/lib/generated/bindings.ts`（`mise run types` で生成）
- Node ⇔ Hub のメッセージ: `crates/fxg-protocol/src/node_hub.rs`
- Client API の型: `crates/fxg-protocol/src/client_api.rs`

このドキュメントでは、型の全文コピーではなく、境界・不変条件・利用者が
誤解しやすい契約を説明します。

---

## 1. セッションイベントモデル

### 1.1 エンベロープと順序

`SessionEventEnvelope` は次の情報を持ちます。

| フィールド   | 意味                                                                   |
| :----------- | :--------------------------------------------------------------------- |
| `event_id`   | UUID v7。冪等適用と重複排除のキー。                                    |
| `session_id` | セッション ID（UUID v7）。                                             |
| `node_seq`   | セッション内の連番（1, 2, 3...）。実行ノードだけが採番し、欠番はない。 |
| `created_at` | ノードのローカル時計（Unix epoch ms）。                                |
| `payload`    | `UnifiedEventPayload`。イベント本体。                                  |

`SessionEventBatch` はイベント列と `cursor` を持ちます。`cursor` は
**配信元ストア**（`node.db` / `hub.db`）への取り込み順で、クライアントの
差分再開にだけ使います。接続先を変えた場合、別ストアの `cursor` は使えません。

### 1.2 永続化の境界

- ユーザーメッセージ、ツール呼び出し、承認 / 質問、状態変化などの離散イベントは
  発生時に永続化する。
- `AgentMessage` / `AgentThought` はストリーミング中に `LiveStreamDelta` として
  メモリ配信し、完成した `is_complete = true` のイベントだけを永続化する。
- `TerminalInput` はエフェメラルで、イベントログには永続化しない。
- `LiveStreamDelta` は再接続時にリプレイしない。

### 1.3 イベント種別と投影

イベント種別の正は `UnifiedEventPayload::event_type()` と
`crates/fxg-protocol/src/common/audit.rs` の `EventType` です。主なイベントは
次のとおりです。

| イベント                                     | 用途 / ハブ側の投影                                           |
| :------------------------------------------- | :------------------------------------------------------------ |
| `SessionCreated`                             | セッション生成（`node_seq = 1`）。`sessions` 行の生成源。     |
| `SessionTitleChanged`                        | タイトル変更。`sessions.title`。                              |
| `SessionPermissionPolicyChanged`             | 承認ポリシー変更。`sessions.permission_policy`。              |
| `SessionAgentBound`                          | ACP セッション確立。`sessions.agent_session_id`。             |
| `UserMessage`                                | ユーザーのプロンプト（添付・`client_source` を含む）。        |
| `AgentMessage` / `AgentThought`              | エージェントの返答 / 思考。完成イベントだけを永続化。         |
| `ToolCall`                                   | ツール呼び出しと Diff。同一 `tool_call_id` の追記で状態更新。 |
| `PlanUpdate`                                 | 実行計画（ACP Plan）。                                        |
| `PermissionRequest` / `PermissionResolved`   | 承認リクエストと解決。`permission_requests` の投影。          |
| `ElicitationRequest` / `ElicitationResolved` | 構造化入力と解決。`elicitation_requests` の投影。             |
| `TerminalOutput`                             | ターミナル出力。FTS の `searchable_text` からは除外。         |
| `CapabilitiesUpdated`                        | モード・コマンド・設定。`sessions.current_mode` などの投影。  |
| `ProvisioningLog`                            | 一時VMのプロビジョニングログ。                                |
| `UsageUpdated`                               | コンテキスト使用量・コスト。`sessions.usage_json`（最新値）。 |
| `SubagentUpdated`                            | 子セッションの作成・状態更新。`sessions` 行は作らない。       |
| `SubagentMessage`                            | 親子間メッセージ。送信元 / 宛先で方向を保持する。             |
| `TurnEnded`                                  | 正常終了以外のターン終了理由。                                |
| `CompactionUpdated`                          | コンテキスト圧縮の進行状況。                                  |
| `StatusChanged`                              | セッション状態。`sessions.status`。                           |
| `SessionArchived`                            | アーカイブ / 復元。`sessions.archived_at`。                   |
| `SessionDeleted`                             | 削除 tombstone。本文イベントと承認 / 質問をパージする。       |

### 1.4 制御アクション

`SessionControlAction` は `set_mode` / `set_config` / `set_permission_policy` /
`cancel` / `kill` / `dispose` を持ちます。

- `cancel`: 実行中のターンだけを中断する。
- `kill`: セッションを終了し、エージェントプロセスを再利用プールへ返す。
- `dispose`: セッションとプロセスツリーを強制終了する。

### 1.5 エラー

`ErrorCode` の全 variant と UI 文言の対応は `ErrorCode::ALL` と
`ui/messages/en.json` の契約テストで検証します。
`ErrorInfo` は `code` / `message` / `details` / `retryable` / `recovery` /
`data_safety` / `request_id` を運びます。REST / WS のエラー本文は
`{ "error": { "code", "message", ... } }` に統一します。

### 1.6 サブエージェント (subagent) セッション

サブエージェントは fxg の独立セッションではありません。親セッションの
イベントストリームに、子セッション由来のイベントとしてタグ付きで混在させます。
ACP 標準の subagents RFD と opencode の独自拡張（child session updates）は
どちらも同じ正規化イベントへ変換し、UI は発生元を意識しません。

不変条件:

- `SubagentUpdated` は子セッションの作成・メタデータ更新・状態を表す。
  同じ `child_session_id` への追記で更新し、最初のイベントは必ず子の
  他のイベントより先に永続化される。
- 子に由来する `AgentMessage` / `AgentThought` / `ToolCall` /
  `PermissionRequest` / `ElicitationRequest` / `TerminalOutput` /
  `PlanUpdate` / `UsageUpdated` は `subagent_id` を持つ。`subagent_id` の
  ないイベントは親セッションのものとして扱う。
- 子のイベントは親の `sessions` 投影（`status` / `usage_json` /
  `current_mode` / `title`）を書き換えない。親の投影は `subagent_id` を
  持たないイベントだけで更新する。
- サブエージェントは `sessions` 行を持たず、`session/list` にも現れない。
  親セッションをアーカイブ・削除したときは、子のイベントも同じログとして
  扱う。
- `SubagentMessage` は親子間のメッセージを表し、送信元・宛先の
  `session_id` で方向を保持する。ストリーミング中は delta として配信し、
  完成したイベントだけを永続化する（`AgentMessage` と同じ境界）。
- 再接続・ハブ同期・リプレイは通常のイベント列と同じ経路を使う。UI は
  `SubagentUpdated` と `subagent_id` から子のツリーとタイムラインを
  再構築する。

正規化の発生源:

- ACP 標準の `subagents` RFD: 親セッション宛の `subagent_update`、
  子セッション宛の `session/update`、`session_message` /
  `session_message_chunk`。
- opencode: `opencode/session/child_update` 拡張通知。

どちらも `crates/fxg-acp/src/acp/mapping.rs` で同じイベントへ変換します。
型の正は `crates/fxg-protocol/src/events.rs`、UI の型は生成済み
`ui/src/lib/generated/bindings.ts` です。

---

## 2. Node ⇔ Hub 間プロトコル

常駐ノード（Outbound WebSocket）と一時VMノード（`fxg internal stdio` の
JSON Lines）は、トランスポートだけが異なり、同じ `NodeToHubMsg` /
`HubToNodeMsg` を使います。メッセージ型の正は
`crates/fxg-protocol/src/node_hub.rs` です。

### 2.1 トランスポートとバージョン

- **WebSocket**: `wss://<hub-host>/api/v1/node/ws`。
  `Authorization: Bearer <NODE_TOKEN>` で認証し、`NodeHello.node_id` が
  トークン発行対象と一致することを検証する。
- **Stdio**: ハブが子プロセスとして起動したプロビジョナーの `stdin` / `stdout`
  で JSON Lines をやり取りする。初期化ログは `stderr` に出す。
- `NodeHello.protocol_version` は `PROTOCOL_VERSION`。ハブは最小バージョン未満の
  ノードを拒否する。ノードとハブは同一 `fxg` リリースで同時に更新する。

### 2.2 メッセージの分類

- **同期・ストリーム**: `EventBatchPush` / `EventBatchAck`、`ResyncRequest`、
  `LiveStreamDelta`、`PtyOutput` / `PtyExit` / `PtyError`。
  `ResyncRequest` 由来の再送は `EventBatchPush.resync = true` とし、Web Push を
  抑止する。
- **セッションコマンド**: `StartSession`、`SendPrompt`、`RespondPermission`、
  `RespondElicitation`、`ControlSession`。結果は `CommandResult`。
- **汎用 Node RPC**: `NodeRpc` / `NodeRpcResult`。Git、Worktree、プロジェクト、
  エージェント、設定、秘密、セッションのライフサイクルを扱う。
- **ライフサイクル**: `DrainAndShutdown`、`DrainComplete`、`KillAllSessions`。

`NodeHello` は `state_revision` と `projects_complete` を含む全量スナップショットです。
`projects_complete = true` のときだけ、今回のレポートに含まれないバインディングを
prune します。`sessions` は最大 1000 件で、超過時は
`sessions_truncated = true` になります。

### 2.3 汎用 Node RPC

`NodeRpcRequest` は操作ごとの個別メソッドを増やさず、カテゴリごとにまとめています。
タイムアウトは `NodeRpcRequest::timeout()` が正です。

| カテゴリ       | 主な操作                                                                                                            |
| :------------- | :------------------------------------------------------------------------------------------------------------------ |
| Git / Worktree | `GetGitDiff` / `ManageWorktree`（add / remove / prune）                                                             |
| プロジェクト   | `InspectProject` / `AddProject` / `ProjectScan` / `ProjectLink` / `UnlinkProject`                                   |
| エージェント   | `ListAgents` / `ProbeAgentConfig` / `ManageAgent` / `ImportSession`                                                 |
| 設定           | `GetNodeSettings` / `UpdateNodeSettings` / `UpdateAgentSettings` / `UpdatePairing` / `ClearPairing` / `TestPairing` |
| 秘密           | `GetSecrets` / `UpsertSecret` / `DeleteSecret`                                                                      |
| セッション     | `ResumeSession` / `RenameSession` / `ArchiveSession` / `DeleteSession`                                              |

成功時は `NodeRpcOutcome::Ok { response }`、失敗時は
`NodeRpcOutcome::Err { error: ErrorInfo }` を返します。ハブは `ErrorCode` を
文字列に潰さず、HTTP 境界で status に変換します。

---

## 3. Client API

ハブと各ノードのローカル Web サーバーは、同じ API パスと WebSocket 形式を
実装します。型の正は `crates/fxg-protocol/src/client_api.rs` です。

### 3.0 セキュリティ

- `Authorization: Bearer <auth_token>` または
  `Cookie: fxg_session=<token>; HttpOnly; SameSite=Strict`。
- 認証不要: `GET /api/v1/meta`、`GET /healthz`、`GET /readyz`。
- `Host` ヘッダ検証（DNS Rebinding 対策）と、WebSocket の `Origin` 検証
  （CSWSH 対策）。
- 監査ログは、セッション起動、承認 / 質問の解決、kill switch、Worktree 操作、
  プロジェクト紐付け、エージェント管理、ノードトークン、認証トークン再生成、
  設定更新、ペアリング、秘密管理を記録する。プロンプト送信は
  `UserMessage` イベントとして `session_events` に残す。

### 3.1 REST API

以下はリソースごとの概要です。パス・リクエスト / レスポンス型の正は
`crates/fxg-hub/src/api/mod.rs` と `crates/fxg-protocol/src/client_api.rs` です。

#### システム / 認証

| Method / Path                                    | 内容                                                                                                           |
| :----------------------------------------------- | :------------------------------------------------------------------------------------------------------------- |
| `GET /api/v1/meta`                               | 認証不要。`role`（`hub` / `local_node`）を返す。                                                               |
| `GET /api/v1/system/info`                        | `role` / `version` / `vapid_public_key` / `node_id` / `unsynced_event_count` / `hub_link` / `last_synced_at`。 |
| `GET /healthz` / `GET /readyz`                   | 認証不要の liveness / readiness。                                                                              |
| `GET /api/v1/diagnostics`                        | エージェント stderr、ログパス、ヘルス、未解決インシデント。                                                    |
| `GET /api/v1/diagnostics/incidents`              | 永続化されたインシデント一覧（`limit` / `include_resolved`）。                                                 |
| `POST /api/v1/diagnostics/incidents/:id/resolve` | インシデントを解決済みにする。                                                                                 |
| `GET /api/v1/diagnostics/bundle`                 | redaction 済みサポートバンドル。                                                                               |
| `POST /api/v1/auth/login` / `logout`             | トークン入力と Cookie 発行 / 失効。                                                                            |
| `POST /api/v1/auth/rotate-token`                 | クライアント認証トークンを再生成。                                                                             |
| `GET/PATCH /api/v1/settings/hub`                 | ハブ設定（VAPID subject）。                                                                                    |
| `POST /api/v1/system/kill-switch`                | `{ reason }`。全ノードへ `KillAllSessions` を配信し、`{ notified_nodes }` を返す。                             |
| `POST /api/v1/system/shutdown`                   | ハブ / ノードの graceful shutdown。                                                                            |

#### プロジェクト / Worktree / ノード

| Method / Path                                                      | 内容                                                               |
| :----------------------------------------------------------------- | :----------------------------------------------------------------- |
| `GET /api/v1/projects`                                             | 論理プロジェクトとノード / Worktree バインディング。               |
| `GET/POST/DELETE /api/v1/projects/:project_id/worktrees`           | 一覧 / 作成 / 削除。DELETE は body `{ node_id, path, force }`。    |
| `POST /api/v1/projects/:project_id/worktrees/prune`                | `git worktree prune`。body `{ node_id }`。                         |
| `POST /api/v1/nodes/:node_id/projects/inspect`                     | パスのプロジェクト同一性を読み取り専用で解決。                     |
| `POST /api/v1/nodes/:node_id/projects/add`                         | パスを検出してバインディングを登録。                               |
| `POST /api/v1/nodes/:node_id/projects/scan`                        | ディレクトリ配下を一括スキャン。                                   |
| `POST /api/v1/nodes/:node_id/projects/link`                        | 任意ディレクトリを論理プロジェクトへ手動紐付け。                   |
| `POST /api/v1/nodes/:node_id/projects/unlink`                      | バインディングを削除。                                             |
| `GET /api/v1/nodes`                                                | ノード一覧、オンライン状態、一時ノード属性、利用可能エージェント。 |
| `GET /api/v1/nodes/:node_id/fs/browse`                             | ノードのファイルシステム閲覧。                                     |
| `GET /api/v1/nodes/:node_id/settings`                              | ノード設定スナップショット（トークン値は返さない）。               |
| `PATCH /api/v1/nodes/:node_id/settings/node` / `.../agents`        | ノード所有設定の更新。                                             |
| `PUT/DELETE /api/v1/nodes/:node_id/settings/pairing`               | ハブ接続先の保存 / 解除。                                          |
| `POST /api/v1/nodes/:node_id/settings/pairing/test`                | 保存せずに WebSocket ハンドシェイクを試す。                        |
| `GET /api/v1/nodes/:node_id/secrets` / `PUT/DELETE .../:secret_id` | ノード所有の秘密メタデータ。値は返さない。                         |
| `GET /api/v1/hub/secrets` / `PUT/DELETE .../:secret_id`            | ハブ所有の秘密メタデータ。                                         |
| `GET /api/v1/nodes/tokens` / `POST` / `DELETE .../:node_id`        | ノード個別トークンの一覧 / 発行 / 失効（ハブのみ）。               |

#### エージェント

| Method / Path                                                  | 内容                                               |
| :------------------------------------------------------------- | :------------------------------------------------- |
| `GET /api/v1/agents`                                           | ローカルノード専用。`{ agents, registry }`。       |
| `GET /api/v1/nodes/:node_id/agents`                            | 指定ノードのカタログ。ハブではこちらを使う。       |
| `POST /api/v1/nodes/:node_id/agents/refresh`                   | Registry キャッシュを更新。                        |
| `POST /api/v1/nodes/:node_id/agents/:agent_id/probe`           | 一時 ACP セッションで能力を取得。                  |
| `PUT/DELETE /api/v1/nodes/:node_id/agents/custom/:agent_id`    | カスタム ACP エージェント定義の作成・更新 / 削除。 |
| `POST /api/v1/nodes/:node_id/agents/registry/:agent_id/import` | Registry エントリを custom 定義として保存。        |

#### プロビジョナー

| Method / Path                          | 内容                                      |
| :------------------------------------- | :---------------------------------------- |
| `GET /api/v1/provisioners`             | ハブのプロビジョナー一覧。                |
| `PUT /api/v1/provisioners/:name`       | 定義の作成・更新（ハブのみ）。            |
| `DELETE /api/v1/provisioners/:name`    | 定義の削除（ハブのみ）。                  |
| `POST /api/v1/provisioners/:name/test` | 起動と `NodeHello` ハンドシェイクを検証。 |

#### セッション

| Method / Path                                                    | 内容                                                                        |
| :--------------------------------------------------------------- | :-------------------------------------------------------------------------- |
| `GET /api/v1/sessions`                                           | 一覧。`project_id` / `node_id` / `status` / `include_archived` / `limit`。  |
| `POST /api/v1/sessions`                                          | 新規セッション（常駐ノード / プロビジョナー / Worktree / Fork）。           |
| `GET /api/v1/sessions/:id`                                       | 詳細。`recent_events`（既定 20、最大 500）。                                |
| `GET /api/v1/sessions/:id/events`                                | 履歴。`after_cursor` / `before_cursor` / `limit`。                          |
| `POST /api/v1/sessions/:id/prompt`                               | `{ command_id, input, client_source }`。`CommandResult`。                   |
| `POST /api/v1/sessions/:id/control`                              | `{ command_id, action }`。`CommandResult`。                                 |
| `GET /api/v1/sessions/:id/diff`                                  | `scope=uncommitted` / `scope=branch`。                                      |
| `POST /api/v1/sessions/import`                                   | 既存エージェントセッションの取り込み（`mirror_history` で履歴を複製）。     |
| `GET /api/v1/sessions/external`                                  | エージェント側の未取り込みセッション一覧（`node_id` / `agent` / `force`）。 |
| `POST /api/v1/sessions/:id/permissions/:request_id/respond`      | 承認応答。解決済みは `200 { already_resolved: true }`。                     |
| `POST /api/v1/sessions/:id/elicitations/:elicitation_id/respond` | 質問応答。解決済みは `200 { already_resolved: true }`。                     |
| `POST /api/v1/sessions/:id/resume`                               | 停止セッションの再開。                                                      |
| `POST /api/v1/sessions/:id/title`                                | タイトル変更。                                                              |
| `POST /api/v1/sessions/:id/archive`                              | アーカイブ / 復元。                                                         |
| `DELETE /api/v1/sessions/:id`                                    | 削除（tombstone）。                                                         |

#### 検索 / Inbox / Push / 監査

| Method / Path                        | 内容                        |
| :----------------------------------- | :-------------------------- |
| `GET /api/v1/inbox`                  | 未解決の承認 / 質問の一覧。 |
| `GET /api/v1/search?q=...&limit=...` | FTS5 全文検索。             |
| `POST /api/v1/push/subscribe`        | Web Push 購読（ハブのみ）。 |
| `GET /api/v1/audit/logs?limit=...`   | 監査ログ。                  |

### 3.2 Client WebSocket (`/api/v1/client/ws`)

クライアントは接続時に `Subscribe { since_cursor, focused_session_id }` を送ります。
`since_cursor` は接続先ストアの `session_events.cursor` です。

- 上り: `Subscribe` / `SendPrompt` / `ControlSession` / `Ping`。
- 下り: `EventBatch` / `LiveStreamDelta` / `ProvisioningLog` / `CommandResult` /
  `SystemStatus` / `Error` / `Pong`。
- 承認 / 質問の回答、Resume / Archive / Delete は REST 専用。
- 受信イベントは `event_id` / `(session_id, node_seq)` で upsert し、
  `LiveStreamDelta` は `message_id` / `thought_id` でマージする。
- `since_cursor` がストア末尾より大きい場合は無効として 0 から再同期する。
  カーソルの巻き戻りを検知したら、保存済みタイムラインを破棄して REST 投影を
  再取得する。

### 3.3 Client PTY WebSocket (`/api/v1/pty/ws`)

`ghostty-web` などのターミナルとノードの PTY を直結します。

- クライアント: `attach` / `spawn` / `input` / `resize` / `kill`。
- サーバー: `spawned` / `output` / `exit` / `error`。
- ノード設定 `allow_remote_pty = false` のとき、リモートからの `spawn` は
  `PTY_DISABLED` を返す。ローカル端末直結では PTY を利用できる。

---

## 4. CLI ⇔ Local Node

CLI (`fxg`) はローカルノードのループバック HTTP / WS API を使います。
プラットフォーム固有の Local IPC はありません。

- 接続先: `node.addr`（既定 `127.0.0.1:7860`）。7860 が使用中で明示指定が
  ない場合は ephemeral port にフォールバックし、実際のアドレスを
  `node.addr` に書き出す。
- ハブへ直接接続する場合（`--provisioner` / `fxg web hub` 等）は
  `--hub` → `hub.addr` → ノードのペアリング → `[hub] listen_addr` の順で解決する。
- 認証: `~/.flexagent/auth_token` を読み、`Authorization: Bearer` を付与する。
- `node.addr` が stale な場合は `system/info` で疎通を確認し、必要なら
  `fxg node start --detach` 相当で自動起動する。`FXG_NO_AUTO_NODE=1` で無効化する。

CLI のコマンド・引数の正は `fxg --help` / `fxg <noun> --help` と
`__usage_spec__` です。このドキュメントでは個別のコマンド一覧を複製しません。
