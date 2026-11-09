# コードベース全体リファクタリング・モジュール分割計画

- **日付**: 2026-11-09
- **ステータス**: 計画中 (Proposed)
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-acp`, `fxg-node`,
  `fxg-server`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**:
  `mise run fmt` / `mise run lint` / `mise run check` / `mise run types` /
  `cargo test --workspace` / `pnpm --dir ui run check` / `pnpm --dir ui run test`
- **関連ドキュメント**:
  - [`docs/README.md`](../README.md)
  - [`docs/04-agent-drivers-and-windows.md`](../04-agent-drivers-and-windows.md)
  - [`docs/06-type-safety.md`](../06-type-safety.md)
  - [`docs/changelog/2026-11-09-api-protocol-consolidation-plan.md`](2026-11-09-api-protocol-consolidation-plan.md)

---

## 0. 背景と目的

API・プロトコル境界の整理（[`2026-11-09-api-protocol-consolidation-plan.md`](2026-11-09-api-protocol-consolidation-plan.md)）とは別に、各クレート内部の実装において以下の技術的負債が蓄積している。

1. **2,000〜3,600行超の巨大単一ファイル化**:
   1つのファイルにプロセス管理・プロトコル変換・DB操作・ビジネスロジック・単体テストが同居しており、可読性と変更時の見通しが低下している。
2. **クレート間の重複実装（DRY 原則違反）**:
   秘密ストア（`secrets.rs`）、設定の `SourcedValue` 解決ロジック、監査ログ記録・エラー変換などが `fxg-node` と `fxg-server` に重複して実装されている。
3. **フロントエンド (`ui/`) の状態・ロジック集約不足**:
   ページコンポーネント（`routes/projects/+page.svelte` 等）に API 呼び出し・モーダル状態・フォーム状態が直接書かれており、コンポーネントが肥大化している。

---

## 1. クレート横断の重複コード統合 (DRY)

### 1.1 秘密ストア (`secrets.rs`) の `fxg-db` への統合と Key-Value ストアへの再定義

`crates/fxg-node/src/secrets.rs`（392行）と `crates/fxg-server/src/secrets.rs`（208行）に重複していたコードを `fxg-db::secrets` に集約・統合し、あわせて秘密ストアの設計を「任意 Key-Value ストア」に再定義した。

- **解消した歪み**:
  - `SecretKind`（`agent_env` / `git_token`）による用途分類を廃止し、Node / Hub が保持する汎用的な Key-Value 秘密（キー名識別子、任意の説明 `description`、値 `value`）に一本化。
  - プロセス内部の起動資格情報（`node_token` / `auth_token` / `vapid_private`）をユーザー向け秘密ストアの CRUD 対象から分離。
  - 外部 API キー等に対してランダム文字列を上書きしてしまう破綻した `rotate` API / 操作を廃止し、値の更新（PUT）に統一。
  - 不可読な UUID v7 ではなく、ユーザーがつけたキー名（`OPENAI_API_KEY` 等）でエージェント（`KEY=@KEY`）や Git 認証（`KEY`）から直接参照できるように改善。
  - `fxg-node/src/secrets.rs` と `fxg-server/src/secrets.rs` を完全削除し、`fxg-db::secrets` に統合。サーバー側の参照中削除エラーも `ErrorCode::SecretInUse` に統一。

### 1.2 Bootstrap 設定の `SourcedValue` 組み立てロジックの共通化

- **現状の問題**:
  - `crates/fxg-node/src/daemon/mod.rs` と `crates/fxg-server/src/state.rs:167-255` の両方に、環境変数・`config.toml`・CLI引数・デフォルト値の優先順位を判定して `SourcedValue<T>`（`ConfigSource` 付き）を組み立てる `sourced_string` 等のヘルパーが重複している。
- **改善策**:
  - `crates/fxg-protocol/src/config.rs`（または `settings.rs`）に `SourcedValue` の解決ビルダーを集約し、Node / Server 双方から呼び出す。

### 1.3 監査ログ記録 (`AuditLogRecord`) ヘルパーの共通化

- **現状の問題**:
  - `fxg-node` では `DaemonState::record_audit`（`crates/fxg-node/src/daemon/ops.rs:63-82`）と `AuditSource` を使っている一方、`fxg-server` では `state.rs` の各メソッド内で個別に `AuditLogRecord::new(...)` を組み立てて `append_audit_log` を呼んでいる。
- **改善策**:
  - `fxg-db::audit` に共通のビルダー（または `Db::record_audit(...)` メソッド）を用意し、失敗時の `tracing::warn!` を含めて統一する。

---

## 2. 肥大化した単一ファイルのモジュール分割

現在 1,500 行を超えている主要ファイルを、責務ごとのサブモジュールに分割する（外部公開 API は `mod.rs` から `pub use` して維持する）。

### 対象ファイル一覧（行数順）

| ファイル                                                                                                                            | 現在の行数 | 主な混在責務                                                                                                    | 分割後のモジュール構成案                                                                                                                                                                                                                                                                                                                                                                        |
| :---------------------------------------------------------------------------------------------------------------------------------- | ---------: | :-------------------------------------------------------------------------------------------------------------- | :---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`crates/fxg-acp/src/acp.rs`](file:///home/nazo/ghq/github.com/nazo6/flexagent/crates/fxg-acp/src/acp.rs)                           |   3,603 行 | プロセス生成、JSON-RPC クライアントハンドラ、セッション起動/Resume/Fork、設定値変換、テスト(約1,200行)          | `acp/mod.rs` (AcpDriver公開API)<br>`acp/client.rs` (ACP Client トレイト実装・コールバック)<br>`acp/process.rs` (子プロセス・stderr収集・認証検知)<br>`acp/mapping.rs` (ToolCall/Plan/Config/Elicitation 型変換)<br>`acp/tests.rs`                                                                                                                                                               |
| [`crates/fxg-node/src/session_manager.rs`](file:///home/nazo/ghq/github.com/nazo6/flexagent/crates/fxg-node/src/session_manager.rs) |   3,352 行 | セッション管理、500ms チャンク結合フラッシュ、権限ポリシー判定、Elicitation 管理、履歴リプレイ、テスト          | `session_manager/mod.rs`<br>`session_manager/lifecycle.rs` (start / resume / fork / archive / delete)<br>`session_manager/stream.rs` (チャンク結合・500ms フラッシュ・イベント永続化)<br>`session_manager/Approval.rs` (PermissionPolicy / Elicitation 応答)<br>`session_manager/replay.rs` (履歴リプレイ構築)                                                                                  |
| [`crates/fxg-cli/src/commands.rs`](file:///home/nazo/ghq/github.com/nazo6/flexagent/crates/fxg-cli/src/commands.rs)                 |   3,093 行 | 全 CLI サブコマンドの `clap` 定義と実行ハンドラ、出力フォーマッタ、テスト                                       | `commands/mod.rs` (Cli 構造体・エントリ)<br>`commands/run.rs` (`fxg run`, ストリーム表示)<br>`commands/session.rs` (`ps`, `session *`, `inbox`)<br>`commands/project.rs` (`project *`, `worktree *`)<br>`commands/admin.rs` (`agents`, `nodes`, `provisioners`, `auth`, `doctor`)                                                                                                               |
| [`crates/fxg-server/src/api/mod.rs`](file:///home/nazo/ghq/github.com/nazo6/flexagent/crates/fxg-server/src/api/mod.rs)             |   2,077 行 | `ApiError`、`ClientApiBackend` トレイト、Axum ルーター、全 REST ハンドラ、テスト                                | `api/mod.rs` (ルーター・共通型)<br>`api/error.rs` (`ApiError` 変換)<br>`api/backend.rs` (`ClientApiBackend` トレイト)<br>`api/handlers/` (ドメイン別ハンドラまたは RPC ディスパッチャ)                                                                                                                                                                                                          |
| [`crates/fxg-protocol/src/common.rs`](file:///home/nazo/ghq/github.com/nazo6/flexagent/crates/fxg-protocol/src/common.rs)           |   2,063 行 | エラー型、リカバリ型、セッション型、プロジェクト型、Diff型、ACP イベント補助型、監査型                          | `common/mod.rs` (`pub use` 再エクスポート)<br>`common/error.rs` (`ErrorCode`, `ErrorInfo`, `RecoveryAction`, `DataSafety`)<br>`common/session.rs` (`SessionSummary`, `SessionStatus`, `PromptInput`, `SessionControlAction`)<br>`common/project.rs` (`ProjectSummary`, `NodeSummary`, `WorkspaceDiffResponse`)<br>`common/acp.rs` (`Permission*`, `Elicitation*`, `ConfigOption*`, `ToolCall*`) |
| [`crates/fxg-server/src/state.rs`](file:///home/nazo/ghq/github.com/nazo6/flexagent/crates/fxg-server/src/state.rs)                 |   2,009 行 | `ServerState` 初期化、設定管理、トークン管理、セッション作成・Provisioner 起動、`ClientApiBackend` 実装、テスト | `state/mod.rs` (`ServerState` 本体)<br>`state/settings.rs` (ServerSettings / ProjectProfile / Secrets)<br>`state/sessions.rs` (セッション起動・Provisioner 連携・Fork 検証)<br>`state/api_impl.rs` (`ClientApiBackend for ServerState`)                                                                                                                                                         |
| [`crates/fxg-node/src/daemon/ipc.rs`](file:///home/nazo/ghq/github.com/nazo6/flexagent/crates/fxg-node/src/daemon/ipc.rs)           |   1,663 行 | UDS / Named Pipe ソケット待受、フレーミング処理、全 IPC コマンド分岐、テスト                                    | `daemon/ipc/mod.rs` (transport / accept ループ)<br>`daemon/ipc/handlers.rs` (コマンドディスパッチ)                                                                                                                                                                                                                                                                                              |

---

## 3. フロントエンド (`ui/`) のリファクタリング

### 3.1 巨大ページコンポーネントの分割

- [`ui/src/routes/projects/+page.svelte`](file:///home/nazo/ghq/github.com/nazo6/flexagent/ui/src/routes/projects/+page.svelte)（835行）：
  - プロジェクト一覧、ノード別ディレクトリ登録（Scan / Link）、Worktree 一覧・作成・削除・Prune、および Project Profile（Bootstrap 設定）編集ダイアログが1ファイルに詰め込まれている。
  - `ui/src/lib/components/projects/` 配下に `ProjectCard.svelte`, `WorktreeManagerDialog.svelte`, `ProjectProfileDialog.svelte`, `ProjectLinkForms.svelte` として切り出す。

### 3.2 イベント Reducer (`ui/src/lib/sync/reducer.ts`) の整理

- [`ui/src/lib/sync/reducer.ts`](file:///home/nazo/ghq/github.com/nazo6/flexagent/ui/src/lib/sync/reducer.ts)（765行）：
  - タイムライン構築、ストリーミングチャンクのマージ、ツール呼び出し状態更新、Permission/Elicitation 状態更新を1ファイルで処理している。
  - 純粋関数としてのテスト容易性は高いため、アイテム種別ごとのハンドラ関数に整理して見通しを良くする。

---

## 4. 変更波及 (Ripple Effect) を抑制するアーキテクチャ方針

機能追加やスキーマ変更時に変更箇所が全方位に拡散するのを防ぐため、以下の設計施策を合わせて適用する。

### 4.1 テスト用 Fixture ビルダー (Object Mother パターン) の導入

- **現状の課題**:
  - `SessionSummary`, `ProjectSummary`, 各種 Request/Response 型にフィールドを1つ追加・変更した際、各クレート内のテストコードにある直接構造体リテラル初期化（`SessionSummary { session_id, ... }`）が一斉にコンパイルエラーになり、無関係なテスト数十箇所の修正に追われる。
- **改善策**:
  - `fxg-protocol` のテスト支援モジュール（`#[cfg(any(test, feature = "testing"))]`）に、共通のダミー生成・ビルダー（例: `SessionSummary::dummy()`, `ProjectSummary::dummy()`, あるいは `Default` 実装）を整備する。
  - テスト側では検証対象のフィールドのみを明示し、それ以外のフィールドはデフォルト値で吸収させることで、構造体拡張時のテスト修正コストをゼロにする。

### 4.2 UI 状態管理・データフェッチの宣言化 (Query / Resource パターン)

- **現状の課題**:
  - 各ページ（`projects/+page.svelte`, `sessions/+page.svelte` 等）で `loading`, `error`, `busy`、手動再取得（`sync.refreshProjects()`）を命令的に個別実装しており、API の引数や戻り値が変わるたびにコンポーネント内の非同期フロー全体に修正が波及する。
- **改善策**:
  - Svelte 5 の Runes に適したデータフェッチ・キャッシュ無効化の共通機構（軽量な Resource パターンまたは TanStack Query / Svelte Query 等の導入）を整備する。
  - 「セッション作成や Worktree 操作が成功したら関連キーのキャッシュを invalidate する」という宣言的記述に統一し、コンポーネント内のボイラープレートを排除する。

### 4.3 DB Row とプロトコル型の過度な二重化 (マッピング税) の緩和

- **現状の課題**:
  - `SessionRow` と `SessionSummary` のように、DB層（`fxg-db`）とプロトコル層（`fxg-protocol`）でほぼ同一定義の構造体を別々に持ち、`into_summary()` によるフィールド詰め替えを手書きしているため、カラム追加ごとに migration → Row → マッピング関数 → Summary 型と4重の変更が必要。
- **改善策**:
  - 1対1で対応するプロトコル型については `fxg-protocol` の型を `sqlx::FromRow` で直接受けるか、共通マクロでマッピングを自動化する。
  - 将来的に拡張・変更が頻繁に予想されるメタデータや設定項目は、無理に固定カラムにせず `metadata: Option<RawJson>` などの拡張 JSON カラムを活用し、DB マイグレーションと全レイヤーの型修正の頻度を抑える。

---

## 5. 段階的チェックリスト

- [ ] **Phase 1: 重複コードの統合 (DRY)**
  - [x] `fxg-node/src/secrets.rs` と `fxg-server/src/secrets.rs` を `fxg-db::secrets` に統合し、汎用 Key-Value 秘密ストアへ再定義（SecretKind・ランダムRotateの廃止、キー名直接参照、サーバー側 `SecretInUse` 不整合修正）
  - [ ] Bootstrap 設定の `SourcedValue` 構築ヘルパーを `fxg-protocol::config` に共通化
  - [ ] 監査ログ記録ヘルパーを `fxg-db::audit` に共通化
- [ ] **Phase 2: `fxg-protocol` と `fxg-db` のモジュール分割・マッピング簡素化**
  - [ ] `crates/fxg-protocol/src/common.rs` を責務別サブモジュール（`error`, `session`, `project`, `acp`）に分割（`common::*` の再エクスポートにより呼び出し側は非破壊）
  - [ ] 主要エンティティのテスト用 Fixture ビルダー（`SessionSummary::dummy()` 等）を整備
  - [ ] `fxg-db` の Row 構造体と Protocol 型のマッピングボイラープレートを整理
- [ ] **Phase 3: `fxg-acp` と `fxg-node` の巨大ファイル分割**
  - [ ] `crates/fxg-acp/src/acp.rs` を `client`, `process`, `mapping`, `tests` に分割
  - [ ] `crates/fxg-node/src/session_manager.rs` を `lifecycle`, `stream`, `approval`, `replay` に分割
- [ ] **Phase 4: `fxg-server` と `fxg-cli`・`ui` のモジュール分割**
  - [ ] `crates/fxg-server/src/state.rs` と `api/mod.rs` を分割
  - [ ] `crates/fxg-cli/src/commands.rs` をサブコマンド群ごとに分割
  - [ ] `ui/src/routes/projects/+page.svelte` からサブコンポーネントを分離
- [ ] **Phase 5: フロントエンドデータフローの宣言化**
  - [ ] UI のデータフェッチ・キャッシュ無効化を宣言化（Query / Resource パターン導入）
