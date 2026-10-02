# ACP 追加機能 実装計画（重要度順 Phase 1〜5）

- **日付**: 2026-10-02
- **対象パッケージ**: `fxg-acp`, `fxg-protocol`, `fxg-db`, `fxg-node`,
  `fxg-server`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**: `mise run check` / `mise run types` /
  `mise run sqlx:prepare`
- **関連設計**: [`docs/04`](../04-agent-drivers-and-windows.md) §2、
  [`docs/03`](../03-protocol-and-api.md)、[`docs/02`](../02-database-schema.md)

## 背景

`AcpDriver` は主要コールバック（`session/update`・`session/request_permission`・
`fs/*`・`terminal/*`）を実装済みだが、ACP schema 1.9.1 / SDK 2.2.0 との
突き合わせで以下の未実装領域が確定した。

| 領域                                             | 現状                                        | 根拠                                             |
| :----------------------------------------------- | :------------------------------------------ | :----------------------------------------------- |
| elicitation (`elicitation/create`)               | 未実装（capability 広告なし・ハンドラなし） | `crates/fxg-acp/src/acp.rs:887`                  |
| `UsageUpdate`                                    | 破棄（コメントで未対応と明記）              | `crates/fxg-acp/src/acp.rs:1585`                 |
| `StopReason` の意味付け                          | debug ログのみ                              | `crates/fxg-acp/src/acp.rs:1137`                 |
| 認証 (`authMethods` / `authenticate` / `logout`) | 未実装                                      | `crates/fxg-acp/src/acp.rs:900` 付近             |
| `ToolCallContent::Content` / `Terminal`          | 破棄（Diff のみ抽出）                       | `crates/fxg-acp/src/acp.rs` `tool_call_contents` |
| 添付・マルチモーダル                             | 送受信とも未対応                            | SDK `session.rs:1035`                            |

- elicitation・`UsageUpdate`・`authMethods` はすべて stable v1（feature gate
  なし）。SDK 側も `CreateElicitationRequest` の `on_receive_request` 登録を
  既定 feature で受けられる（`schema/agent_to_client/requests.rs`）。
- 各 Phase は独立してリリース可能とし、Phase 内は「1 トピック 1
  コミット」で進める。
- 型追加のたびに `mise run types`（ts-rs 出力）、SQL 追加のたびに
  `mise run sqlx:prepare`（`.sqlx/` をコミット）を必須とする。
- 実装完了ごとに対応する `docs/02` / `docs/03` / `docs/04` の節を更新する。

## Phase 一覧

| Phase       | 内容                                                       | 主な対象                                       | 依存 | 状態   |
| :---------- | :--------------------------------------------------------- | :--------------------------------------------- | :--- | :----- |
| **Phase 1** | elicitation（質問/構造化入力）対応                         | protocol / db / acp / node / server / cli / ui | -    | 未着手 |
| **Phase 2** | セッション状態の可視化（UsageUpdate / StopReason）         | acp / protocol / db / ui / cli                 | -    | 未着手 |
| **Phase 3** | エージェント認証（authenticate / logout / terminal）       | acp / node / cli / ui                          | -    | 未着手 |
| **Phase 4** | ツール出力・添付コンテンツ（ToolCallContent / multimodal） | acp / protocol / node / ui                     | -    | 未着手 |
| **Phase 5** | 整合性の細部と unstable 対応                               | acp / db / node                                | -    | 未着手 |

> **参考: permission 系は elicitation と同型の先行実装がある。新機能はこれを
> ミラーする。**
>
> - イベント: `crates/fxg-protocol/src/events.rs`
>   （`PermissionRequest` / `PermissionResolved`）→ 投影
>   `crates/fxg-db/src/events.rs`
> - テーブル: `permission_requests`
>   （`crates/fxg-db/migrations/0001_initial.sql:128`）
> - pending 管理: `crates/fxg-node/src/session_manager.rs`
>   （`pending_permissions` / `respond_permission`）
> - REST / WS: `crates/fxg-server/src/api/mod.rs`
>   （`ClientApiBackend` / `ClientCommand` / `client_router`）と両実装
>   （`crates/fxg-server/src/state.rs`, `crates/fxg-node/src/daemon/api.rs`）
> - Push: `crates/fxg-server/src/push.rs`
>   （`notify_permission_requests`）と `crates/fxg-server/src/hub.rs`
> - UI: `ui/src/lib/sync/reducer.ts` / `PermissionCard.svelte` /
>   `ui/src/routes/inbox/`
> - CLI: `fxg inbox list|approve|reject`（`crates/fxg-cli/src/commands.rs`）

---

## Phase 1: elicitation（質問/構造化入力）

**目的**: ACP の `elicitation/create`（form モード）を実装し、エージェントの
「質問」ツールへ Web / Android / CLI から回答できるようにする。

**ACP 仕様の必須要件（実装時に厳守）**

- `clientCapabilities.elicitation.form` を広告しない限りエージェントは要求
  できない（未広告モードの要求はエージェント側の責任で `-32602`）
- 応答は `accept` / `decline` / `cancel` の 3 択。`accept` は `content`
  （schema 準拠）を返す
- クライアント MUST: 送信前の内容確認・編集、decline / cancel の明示、form で
  秘密情報を扱わない
- `elicitation/complete` 通知は未知 / 完了済み ID を無視する

### 1-1. `fxg-protocol`（型・イベント・API）

- [ ] `common.rs`: `ElicitationRequestEntry`（inbox 表示用: id / session_id /
      message / mode / schema / created_at）を追加。`accept` の content は
      `serde_json::Value` で保持し、schema 準拠の型安全は UI 側で担保する
- [ ] `events.rs`: `ElicitationRequest { elicitation_id, message, mode,
      requested_schema, tool_call_id }` / `ElicitationResolved {
      elicitation_id, action, content, resolved_by }` を追加
      （`is_persistable = true`、`event_type` は `elicitation_request` /
      `elicitation_resolved`）
- [ ] `client_api.rs`: `InboxResponse` に `elicitations:
      Vec<ElicitationRequestEntry>` を追加 / `RespondElicitationRequest {
      action, content }` / `RespondElicitationResponse { already_resolved }` /
      `ClientWsMessage::RespondElicitation`
- [ ] `ipc.rs`: `IpcClientMessage::RespondElicitation` と inbox 応答への追加
- [ ] `mise run types` で `ui/src/lib/generated` を同期

### 1-2. `fxg-db`（投影・クエリ）

- [ ] `migrations/0005_elicitation_requests.sql`: `permission_requests` と
      同型のテーブル（`elicitation_id` PK / `session_id` / `node_id` /
      `message` / `mode` / `schema_json` / `tool_call_id` / `status`
      （pending / accepted / declined / cancelled）/ `content_json` /
      `created_at` / `resolved_at` / `resolved_by`）+ pending partial index
- [ ] `events.rs`: 2 イベントの投影（INSERT ... ON CONFLICT / UPDATE
      status）、全量再構築（`rebuild_*`）、`SessionDeleted` 時のパージ
- [ ] `queries.rs` / `db.rs`: `pending_elicitations` /
      `find_elicitation_request`
- [ ] `searchable.rs`: 質問文を検索対象に含める（permission と同様）
- [ ] `mise run sqlx:prepare` → `.sqlx/` 更新

### 1-3. `fxg-acp`（ドライバ）

- [ ] `acp.rs`: capability に `elicitation(ElicitationCapabilities::new()
      .form(ElicitationFormCapabilities::new()))` を追加（URL モードは広告
      しない）
- [ ] `ElicitationRegistry`（`PermissionRegistry` と同型の oneshot Map）を
      追加
- [ ] `on_receive_request(CreateElicitationRequest)`: セッション context 解決
      → `StatusChanged`（status は未決事項参照）→ `ElicitationRequest` イベント
      → oneshot 登録 → 応答待ち
- [ ] `ActiveSessionHandle::respond_elicitation(request_id, action, content)` を
      追加（トレイト既定実装「未対応」+ `AcpSessionHandle` 実装 +
      `driver.rs` の `MockHandle` 更新）
- [ ] `session/cancel` 時・teardown 時に pending elicitation を `cancel` で
      解決（仕様 MUST）
- [ ] `on_receive_notification(CompleteElicitationNotification)`: 未知 ID を
      無視（form のみの間はログのみ）
- [ ] `accept` 時の content を schema に対して検証（クライアント SHOULD）

### 1-4. `fxg-node`（pending 管理・冪等性）

- [ ] `session_manager.rs`: `PendingElicitationSummary` と
      `ActiveSession.pending_elicitations` を追加
- [ ] ドライバイベント処理（現在の `PermissionRequest` 処理箇所）に
      elicitation の登録 / 解除を追加
- [ ] `SessionManager::respond_elicitation`（解決済みは
      `NodeError::AlreadyResolved` → `ALREADY_RESOLVED`）
- [ ] CLI 向け IPC ディスパッチ（`RespondElicitation`）

### 1-5. `fxg-server`（共通 API・Push）

- [ ] `api/mod.rs`: `ClientApiBackend::respond_elicitation`、REST
      `POST /api/v1/sessions/:id/elicitations/:elicitation_id/respond`、
      WS op、`ClientCommand::RespondElicitation`
- [ ] `state.rs`（`ServerState`）: ノード転送実装（`respond_permission` と
      同型）
- [ ] `daemon/api.rs`（`DaemonState`）: ローカル実装
- [ ] `push.rs`: `notify_elicitation_requests`（通知タップでアプリを開く。
      form の直接回答は Push からは不可）。`hub.rs` の該当箇所から呼ぶ

### 1-6. `ui`（回答フォーム）

- [ ] `sync/reducer.ts`: `kind: "elicitation"` の追加と
      `elicitation_resolved` のマージ（解決後も質問カードを残す）
- [ ] `ElicitationCard.svelte`: `requested_schema` からフォーム生成
      （string / enum・oneOf / boolean / integer・number（min・max）/
      multi-select array、required、default の事前入力、送信前レビュー）
- [ ] `ChatTimeline.svelte` / `inbox/+page.svelte` / `Composer.svelte`
      （回答待ち中の送信制御）/ `session-status.ts`（status ラベル）
- [ ] `stores/sync.svelte.ts`: `respondElicitation`（WS / REST）
- [ ] `service-worker.ts`: 通知タップ → セッションを開く（直接応答はしない）

### 1-7. `fxg-cli`

- [ ] `fxg inbox list` に質問を表示（種別列 or セクション分け）
- [ ] `fxg inbox answer <id>`: `--accept key=value,...` / `--decline` /
      `--cancel`。TTY では対話選択（enum は番号選択、自由入力は簡易エディタ）
- [ ] `tui.rs`: pending 質問ブロックと回答キー（承認と同型 + 自由入力）

### 1-8. テスト

- [ ] `fxg-db`: 投影 / 全量再構築 / tombstone パージの単体テスト
- [ ] `fxg-node`: `MockDriver` で elicitation イベント → pending → 応答 →
      2 回目 `ALREADY_RESOLVED`（permission テストと同型）
- [ ] `fxg-acp`: 応答レジストリ・content 検証・cancel 時の一括キャンセルの
      単体テスト
- [ ] `ui`: reducer / カードの vitest
- [ ] 可能なら SDK の Agent 側を in-process（`tokio::io::duplex` 等）で
      接続した E2E を検討（調査項目）

**完了条件 (DoD)**: テスト用エージェントが送った elicitation が UI / CLI に
表示され、accept（content 付き）/ decline / cancel のすべてがエージェントへ
正しく返る。Push 通知と inbox 表示が機能し、`mise run check` が通る。

**未決事項（実装時に確定）**

- 質問待ちの status: `SessionStatus::WaitingPermission` を再利用するか
  `WaitingInput` を追加するか（**推奨: `WaitingInput` 追加**。UI 表示が
  「承認待ち」では誤解を招く）
- URL モード: セキュリティ要件（ホスト表示・同意・prefetch 禁止・専用
  ブラウザコンテキスト）が重いため本 Phase では広告しない。必要時に別
  Phase とする

---

## Phase 2: セッション状態の可視化

### 2-1. `UsageUpdate`（コンテキスト使用量・累積コスト）

**目的**: コンテキスト残量と累積コストを UI / CLI に表示する（stable v1）。

- [ ] `fxg-protocol`: `UsageCost { amount, currency }` と
      `UnifiedEventPayload::UsageUpdated { used_tokens, context_size, cost }`
      を追加
- [ ] `fxg-acp`: `map_session_update` の `SessionUpdate::UsageUpdate` を
      `UsageUpdated` へ変換（現在は `other => trace!` で破棄）
- [ ] `fxg-db`: `sessions` に `usage_json` を追加（migration 0006）+ 投影 +
      再構築
- [ ] `ui`: コンテキストメーター（Composer / ヘッダ）とコスト表示
- [ ] `fxg-cli`: `fxg session show` 等への表示
- [ ] opencode2 に相当情報があるか調査し、あれば同一イベントへマップ
      （無ければ ACP のみ対応と明記）

### 2-2. `StopReason` の意味付け

**目的**: `Refusal` / `MaxTokens` / `MaxTurnRequests` をユーザーに見せる
（現在は debug ログのみ）。

- [ ] `fxg-protocol`: `TurnEnded { reason, message }` 相当のイベントを追加
      （`EndTurn` / `Cancelled` は従来どおりイベントなし）
- [ ] `fxg-acp`: `serve_session` で stop_reason を判定してイベント化
- [ ] `ui` / `fxg-cli`: タイムラインにシステム行として表示
- [ ] テスト: stop_reason ごとの分岐

---

## Phase 3: エージェント認証（authMethods / authenticate / logout）

**目的**: 認証必須のエージェント（`authMethods` を広告し `auth_required` を
返すもの）を扱えるようにする。

- [ ] `fxg-acp`: `InitializeResponse.auth_methods` を保持し、
      `authenticate(methodId)`（agent 型）を送れる API を追加
- [ ] `fxg-acp`: `clientCapabilities.auth.terminal = true` を広告（terminal
      型メソッドは広告時のみエージェントが出せる）。terminal 型は fxg PTY で
      エージェント本体を対話実行し、exit 0 で成功・再接続（`docs/04` の
      純正 TUI 実行基盤を流用）
- [ ] `fxg-node` / `fxg-cli`: `fxg agents login <agent> [--method <id>]` を
      追加（メソッド一覧表示 → 選択 → 実行）。`logout` は
      `agentCapabilities.auth.logout` 広告時のみ `fxg agents logout`
- [ ] `session/new` の `auth_required` エラーを判別してログイン導線を案内
      （`ErrorCode` 追加）
- [ ] `ui`: エラー時のログイン案内（コマンド表示）
- [ ] テスト: 擬似認証エージェントでのフロー

---

## Phase 4: ツール出力・添付コンテンツ

### 4-1. `ToolCallContent` の充実

- [ ] `fxg-acp`: `tool_call_contents` で `Content`
      （`ContentBlock::Text`）を抽出し、
      `StreamDeltaPayload::ToolCallProgress` / `raw_output` へマッピング
      （Diff のみの現状を解消）
- [ ] `fxg-acp`: `Content`（Image / Resource）は当面ログのみ（表示経路は
      4-2 と同時に設計）
- [ ] `fxg-acp`: `ToolCallContent::Terminal` の `terminal_id` を `ToolCall`
      イベントへ関連付け（`terminal/*` の出力とツールカードを紐付け）
- [ ] `fxg-protocol`: `ToolCall` へ `terminal_id: Option<String>` を追加
      （ts-rs 再生成）
- [ ] `ui`: ツールカードからターミナル出力へのリンク

### 4-2. マルチモーダル（添付の送受信）

**制約**: SDK `ActiveSession::send_prompt(impl ToString)` は単一 Text block
のみ（`session.rs:1035`）。

- [ ] 送信: `ActiveSessionHandle::send_prompt` を添付対応に拡張
      （`AttachmentMeta` を `ContentBlock::Image` / `ResourceLink` へ変換）。
      SDK の `session.connection()` から `PromptRequest` を直接送る方式へ
      `serve_session` を再構成（`PromptResponse` を自前で受ける）か、SDK
      上流へ multi-block API を提案
- [ ] `fxg-node`: `send_prompt` API / IPC に添付を通す（現状
      `session_manager.rs:1742` で空固定）
- [ ] 受信: `AgentMessageChunk` / `AgentThoughtChunk` の Text 以外
      （Image / Audio / ResourceLink / Resource）を破棄せずイベント化
      （`acp.rs:1491` / `:1509`）
- [ ] `ui`: 添付の表示と Composer からの送信
- [ ] テスト: 画像付きプロンプトの往復

---

## Phase 5: 整合性の細部と unstable 対応

- [ ] `session.config_options.boolean` capability を広告（boolean 設定の
      表示・変更には対応済みだが、広告が無いとエージェントは boolean
      オプションを出せない）
- [ ] `terminal/output` の `truncated` を正しく返す（`output_byte_limit` で
      切り詰めた場合 `true`。現状は常に `false`）
- [ ] `NewSessionRequest.additional_directories` 対応（追加ワークスペース
      ルート。設定で複数ルートを持つ場合）
- [ ] `session/close` をセッション終了時に送る（エージェント側リソース
      解放）。`session/delete` は `fxg session delete` との整合を確認して
      判断（`session/list` は使わない方針を docs に明記）
- [ ] unstable 対応の可否決定: `unstable_plan_operations`（PlanUpdate /
      PlanRemoved）、`unstable_session_notices`（Notice）、
      `unstable_session_compaction`。必要になるまで着手しない（現状は stable
      の `Plan` のみ対応）
- [ ] `unstable_session_fork` feature を `Cargo.toml` から外す
      （`ForkSessionRequest` は未使用。fxg の Fork は Shadow Git + Worktree +
      履歴 Replay）
- [ ] `docs/02` / `docs/03` / `docs/04` の最終更新

---

## 実装順序とコミット粒度

- Phase 1 は protocol（1-1）→ db（1-2）→ acp（1-3）→ node（1-4）→
  server（1-5）→ client（1-6 / 1-7）→ test（1-8）の順。各サブフェーズで
  `mise run check` を通してコミットする。
  - 例: `feat(fxg-protocol): elicitation イベントと応答 API を追加` →
    `feat(fxg-db): elicitation_requests 投影を追加` →
    `feat(fxg-acp): elicitation/create ハンドラを実装` →
    `feat(fxg-node): elicitation の pending 管理と冪等応答` →
    `feat(fxg-server): elicitation 応答 API と Web Push` →
    `feat(ui): elicitation フォームと inbox 対応` →
    `feat(fxg-cli): fxg inbox answer を追加`
- Phase 2〜5 はそれぞれ独立に着手可能。Phase 2-1・2-2、Phase 5 の各項目も
  独立コミット。
- `fxg-protocol` の型追加時は同一コミット内で `mise run types` の生成物を
  含める。SQL 追加時は `mise run sqlx:prepare` の `.sqlx/` 更新を含める。

## 検証

- `mise run check`（fmt → clippy → テストの直列実行）
- 型同期: `mise run types` 後に `git status --short ui/src/lib/generated` で
  想定外差分が無いこと
- SQL: `mise run sqlx:prepare` 後に `.sqlx/` をコミット
- UI 単体: reducer / カードの `vitest`
- 手動 E2E: 擬似 ACP エージェント（elicitation を送るテストフィクスチャ）で、
  Web / CLI 双方からの回答を確認

## リスクと未決事項

- form schema の UI 対応範囲（制限 JSON Schema: string / enum / oneOf /
  boolean / integer / number / multi-select array で全網羅の見込み）
- Push からは form の直接回答ができない（通知タップのみ）。URL モード導入時に
  直接操作を再検討
- マルチモーダルは SDK 制約（`send_prompt` 単一ブロック）があり、ターン
  ループの再構成が必要（Phase 4 の設計判断）
- 質問の DB 保持期間: permission と同様に tombstone 時に削除し、それ以外は
  保持
- 実エージェント依存: 一部エージェントは elicitation 未実装の可能性があり、
  検証はテストフィクスチャ主体とする
