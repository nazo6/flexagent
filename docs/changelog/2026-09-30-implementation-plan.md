# FlexAgent (`fxg`) 段階的実装プラン & 進捗記録

- **日付**: 2026-09-30
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-pty`, `fxg-acp`,
  `fxg-node`, `fxg-server`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**: `Cargo.toml`, `mise.toml`, `ui/package.json`

---

## 運用ルール（初期実装期間中）

- 初期実装（Phase 1 〜 Phase 6）の期間中は、新しい changelog
  ファイルを作成せず、**本ファイル
  (`docs/changelog/2026-09-30-implementation-plan.md`)
  をマスタープラン兼進捗記録として参照・順次更新**すること。
- 各構造体・SQLスキーマ・プロトコル・UI仕様の詳細定義は本ファイルに重複記載せず、必ず以下の**元設計ドキュメント**を直接参照して実装すること：
  - [`docs/README.md`](../README.md): コアコンセプト・クレート構成
  - [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md):
    全体トポロジー、Outbox同期、論理プロジェクト・Worktree解決、一時VM
    (`--stdio`)、セキュリティ
  - [`docs/02-database-schema.md`](../02-database-schema.md): `node.db` /
    `server.db` 共通の単一 SQLite スキーマ定義・投影ルール・FTS5
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md): `fxg-protocol`
    型定義、WS/Stdio/IPC メッセージ、REST API エンドポイント
  - [`docs/04-agent-drivers-and-windows.md`](../04-agent-drivers-and-windows.md):
    `AgentDriver`、ACP / `opencode2` 統合、Shadow Git Revert/Fork、Windows
    固有実装
  - [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md): CLI
    コマンド完全リファレンス、設定ファイルスキーマ (`config.toml` /
    `.fxg.toml`)、PWA / Web Push 設計、ターミナル抽象化 (`ITerminalAdapter`)
- **依存関係は自分の記憶に頼らず、常に最新のクレート/ライブラリを確認して導入する**:
  - Rust: `cargo add <crate>` で crates.io
    の最新版を解決して追加し、必要に応じて `cargo outdated` で更新確認。使用前に
    docs.rs の該当バージョンの API を確認する
  - UI: `pnpm add <pkg>` / `pnpm outdated` を使用し、`package.json`
    のバージョンを手打ちしない
  - `Cargo.lock` / `pnpm-lock.yaml` は常にコミットする
- **`fxg-db` の SQL は sqlx の型安全クエリマクロを必須とする**:
  - `sqlx::query!` / `sqlx::query_as!` /
    `sqlx::query_scalar!`（コンパイル時検証）を使用し、 ランタイム文字列の
    `sqlx::query()` / `query_as()` は原則禁止
  - 開発時は `DATABASE_URL` を設定し、CI / オフラインビルド用に
    `cargo sqlx prepare` で生成した `.sqlx/`
    をコミットする（`SQLX_OFFLINE=true`）
  - 動的 IN 句や FTS5 `MATCH` 等でマクロが使えない例外的ケースのみ、
    理由コメントと単体テストを必須とする
- **CLI パースには `usage-rs` を使用する**（`clap` は使用しない）:
  - `usage = { package = "usage-rs", version = "6", features = ["completions"] }`
    （dev-dependencies に `features = ["test"]`）
  - `#[derive(Cli)]` / `#[derive(Args)]` / `#[derive(Subcommands)]` + `Run` /
    `RunWith`（async コマンド）で定義
  - `__usage_spec__` から KDL spec を出力し、`usage` CLI で manpage / Markdown
    リファレンス / シェル補完を生成して
    [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md) §1
    のコマンドリファレンスと同期する
- **コミットは適切なタイミングで行う**:
  - 「フェーズ内のタスク項目が1つ完了した」「1トピックの変更が
    fmt/lint/テストを通った」 時点で、1トピック1コミットでコミットする
  - コミット前に `cargo fmt` / `oxfmt` と該当チェック（`cargo clippy` /
    `cargo test` / `svelte-check` / `oxlint` / `vitest`）を通す
  - 大きめのリファクタや複数ファイルにまたがる変更の着手前に、直前までの作業を
    コミットしておく（並行作業による巻き戻り対策）
  - メッセージは Conventional Commits（例: `feat(fxg-db): ...`,
    `fix(fxg-node): ...`, `docs(plan): ...`）

---

## 実装フェーズ一覧

| フェーズ    | 対象領域                                                     | 主な対象パッケージ                        | 状態              |
| :---------- | :----------------------------------------------------------- | :---------------------------------------- | :---------------- |
| **Phase 1** | ワークスペース・共通プロトコル・設定スキーマ・DB基盤         | `fxg-protocol`, `fxg-db`                  | 完了 (2026-09-30) |
| **Phase 2** | プロセス/PTY制御・Windows対応・ローカルノード基盤            | `fxg-pty`, `fxg-node`, `fxg-cli`          | 完了 (2026-09-30) |
| **Phase 3** | エージェントドライバ (ACP / OpenCode2)・CLI/TUI・Revert/Fork | `fxg-acp`, `fxg-node`, `fxg-cli`          | 未着手            |
| **Phase 4** | 中央サーバー・Outbox同期・Client API共通化・LANセキュリティ  | `fxg-server`, `fxg-node`, `fxg-cli`       | 未着手            |
| **Phase 5** | Web UI / Android PWA・Web Push・単一バイナリ統合             | `ui`, `fxg-server`, `fxg-node`, `fxg-cli` | 未着手            |
| **Phase 6** | 一時VM・サンドボックスノード (`--stdio` & Zero-Touch構築)    | `fxg-node`, `fxg-server`, `fxg-cli`, `ui` | 未着手            |

## 設計レビュー反映履歴

- **2026-09-30**: 初期実装前レビューに基づき、以下を設計ドキュメント (01〜05)
  および本計画へ反映済み:
  - イベント永続化はターン完了時のみ（ストリーミング途中はメモリ配信のみ。切断時の中間ロスは許容）
  - `node.db` の API/スキーマ完全対称化（FTS5 / `permission_requests` /
    `audit_logs` / `snapshot_tree_hash` / Fork系カラム）と `local_seq` 導入
    （※同日の「DB/同期モデル簡素化レビュー」で単一スキーマへ改訂済み）
  - ノード個別 `node_token`（`nodes.token_hash`、発行/失効 CLI）
  - 承認 API の統一 (`POST .../permissions/:req_id/respond`)、二重承認の冪等化
    (`ALREADY_RESOLVED`)、共通エラーコード `ErrorCode`
  - `CommandResult` のクライアント相関返却、オフライン時 `NODE_OFFLINE`、busy 時
    `SendPrompt` の Pending Queue
  - Shadow Git Index のセッション単位分離 (`<session-id>.index`)
  - プロビジョナーは中央サーバーホストで起動（CLI はサーバー API
    経由・サーバー必須）、Bootstrap 短命トークン認証、ベストエフォート Drain
  - 監査ログの server.db / node.db 双方記録、`fxg server` の `0.0.0.0`
    バインドは LAN/VPN 限定運用の注記整理
- **2026-09-30 (DB/同期モデル簡素化レビュー)**:
  ローカルファースト要件（中央サーバー停止時もローカルで継続）を維持したまま、
  「スキーマ1本・ログ1本・権威1つ」の3原則で設計の重複を排除:
  - **スキーマ1本化**: `node.db` / `server.db`
    を同一DDL・同一テーブル名・単一マイグレーションセットへ統一（`local_sessions`
    / `local_session_events` を廃止）。役割差は行スコープと実行ロールのみ
  - **同期のイベントログ一本化**: セッションメタデータを `SessionCreated` /
    `SessionTitleChanged` / `SessionAgentBound` イベント化し、`SessionUpsert` /
    `metadata_synced` を廃止。ハブ側 `sessions` / `permission_requests`
    はイベント適用の投影（同一トランザクション更新・全量再構築可能）
  - **水位ACK**: 行単位 `synced` フラグ + partial index
    を廃止し、`sessions.synced_up_to_node_seq` による水位管理へ
  - **カーソル統一**: `global_seq` / `local_seq` を各DBの
    `session_events.cursor` に統一し、`Subscribe { since_cursor }` +
    `SessionEventBatch { events, cursor }`
    へ変更（`SessionEventEnvelope.global_seq` 削除）
  - **書き込み権威の一元化**:
    セッション状態の書き込み権限は実行ノードのみ（中央サーバーはコマンド転送 +
    イベント適用のみ）
  - **ハンドシェイク再同期**: `NodeHello` の `SessionSyncState` 一覧と
    `ResyncRequest` によるハブDB再構築時の自動復元
  - **`snapshot_tree_hash` 専用カラム廃止**: `UserMessage` payload を正とする
- **2026-09-30 (実装規約の追加)**:
  - 依存関係は自分の記憶に頼らず、常に最新のクレート/ライブラリを確認して導入する（`cargo add`
    / `pnpm outdated`）
  - `fxg-db` の SQL は `sqlx` 型安全クエリマクロ (`query!` / `query_as!` /
    `query_scalar!`) を必須化し、`.sqlx/` (`cargo sqlx prepare`) をコミット
  - CLI パースに `usage-rs` を採用（`clap` 不使用。`__usage_spec__`
    から補完・manpage・Markdown を生成）
  - タスク完了単位のコミット（fmt / lint / テスト通過後、Conventional
    Commits）を運用ルールに追加
- **2026-09-30 (Phase 1 実装時の仕様拡張)**:
  - `SessionCreated` payload に `node_id` を追加（実行ノード自身が埋める）。
    ハブ側 `sessions.node_id` 投影をイベントログのみから再構築可能にするため
    (`docs/02` §0.3 / `docs/03` §1 に反映)
  - `sessions` / `permission_requests` 投影の再構築手順を明確化:
    `session_events` の `ON DELETE CASCADE` により `sessions` 行は DELETE
    できないため、「派生カラムの既定値リセット +
    カーソル順の全量再生」で再構築する (イベント由来でない `git_bundle_path` /
    `synced_up_to_node_seq` は保持)

---

## Phase 1: ワークスペース構築・共通プロトコル (`fxg-protocol`)・DBレイヤー (`fxg-db`)

- **目的**:
  全クレートとUIが参照する「型」「設定スキーマ」「SQLiteスキーマ」を最初に確定させ、後続フェーズでの型変換の書き直しや仮構造体の作成を排除する。
- **参照ドキュメント**:
  - クレート構成: [`docs/README.md` §リポジトリ・クレート構成](../README.md)
  - イベントID・順序・カーソル設計 (`node_seq` / `cursor` / 水位ACK)・
    イベントソーシング投影・永続化ライフサイクル:
    [`docs/01-architecture-and-sync.md` §2.1〜§2.4](../01-architecture-and-sync.md)
  - 単一SQLiteスキーマ (`node.db` / `server.db` 共通)・投影ルール・FTS5:
    [`docs/02-database-schema.md` §0〜§2](../02-database-schema.md)
  - プロトコル・通信メッセージ・IPC型 (`ts-rs`):
    [`docs/03-protocol-and-api.md` §1〜§4](../03-protocol-and-api.md)
  - 設定ファイルスキーマ (`config.toml` / `.fxg.toml`) & `~/.flexagent/` 構成:
    [`docs/05-cli-and-pwa-ui.md` §2.1〜§2.3](../05-cli-and-pwa-ui.md)
- **タスクリスト**:
  - [x] Cargo Workspace (`Cargo.toml` 全7クレート構成) と
        `mise.toml`（ビルド・`ts-rs` 型出力・lint・format タスク）の初期構築。
        依存クレートは `cargo add` で最新版を解決して追加する（`sqlx`, `tokio`,
        `axum`, `usage-rs` 等。バージョンを記憶で手書きしない） →
        完了。`sqlx 0.9` / `ts-rs 12` / `tokio 1.53` 等を `cargo add` で解決し
        `[workspace.dependencies]` に集約。`axum` / `usage-rs` は使用フェーズ
        (Phase 2 / 4) で追加する（未使用依存を持たない）
  - [x] `fxg-protocol`: `SessionEventEnvelope` および `UnifiedEventPayload`
        の定義 (`ts-rs` derive 付与。イベント永続化ライフサイクル:
        ストリーミング途中は非永続、ターン完了時に完成イベントのみ永続化) →
        完了。`is_persistable()` と `event_type()` を payload 側に集約し、 DB の
        `event_type` 語彙と永続化規則の二重定義を排除
  - [x] `fxg-protocol`: Node ⇔ Server メッセージ (`NodeToServerMsg`,
        `ServerToNodeMsg`) の定義
  - [x] `fxg-protocol`: Client REST API リクエスト/レスポンス型、Client WS / PTY
        WS メッセージ型、Local IPC メッセージ型の定義
  - [x] `fxg-protocol`: 共通補助型一式（`SessionSummary`, `NodeProjectReport`,
        `StreamDeltaPayload`, `AttachmentMeta`, `FileDiff`, `PlanEntry`,
        `PermissionOption`, `ModeInfo`, `CommandInfo`, `ConfigOptionInfo`,
        `SessionStatus`, `SessionControlAction`, `WorktreeAction`, `DiffScope`,
        `WorkspaceDiffResponse`, `ForkHistoryItem`）と共通エラーコード
        (`ErrorCode`) の定義
  - [x] `fxg-protocol`: グローバル設定 (`~/.flexagent/config.toml` + `FXG_*`
        環境変数オーバーライド) およびプロジェクト設定 (`.fxg.toml`)
        のデシリアライズ構造体定義（クライアントに公開する型のみ
        `#[ts(export)]`、設定等の内部型は export 対象外） → 環境変数参照は
        `EnvLookup` 注入とし、実環境 (`process_env`) と
        テストで差し替え可能にした
  - [x] `fxg-db`: **単一マイグレーションセット** (`migrations/`)
        による共通スキーマ (`nodes` (個別 `token_hash`) / `projects` /
        `project_node_bindings` / `sessions` / `session_events` (`cursor`
        AUTOINCREMENT) / `session_events_fts` (`tokenize='trigram'`) /
        `permission_requests` / `push_subscriptions` / `audit_logs`)
        の実装。`node.db` / `server.db` は 同一DDLから生成し、ロール（Node /
        Hub）による行スコープ・使用カラムの差のみ扱う → 完了
        (`migrations/0001_initial.sql`)。`PRAGMA journal_mode = WAL` /
        `foreign_keys = ON` / `busy_timeout` はマイグレーション
        (トランザクション内) では効果がないため接続オプションで全接続に適用する
  - [x] `fxg-db`: イベント適用エンジン（`INSERT OR IGNORE` による冪等追記 +
        `sessions` / `permission_requests` 投影の同一トランザクション更新）と
        投影再構築関数（イベントログからの全量再生）の実装 → 投影再構築は
        `session_events` の `ON DELETE CASCADE` により `sessions` を DELETE
        できないため「派生カラムの既定値リセット + カーソル順の
        全量再生」方式。`git_bundle_path` / `synced_up_to_node_seq`
        (イベント由来でない カラム) は保持する
  - [x] `fxg-db`: 水位ベース Outbox 抽出
        (`node_seq > sessions.synced_up_to_node_seq`) と ACK 水位更新の実装 →
        抽出はセッション単位の `SessionOutboxBatch`、ACK は `MAX` による
        単調増加更新。`ResyncRequest` 用の範囲再送 (`extract_outbox_after`) と
        未同期件数 / 最終同期時刻も実装
  - [x] `fxg-db`: クエリ実装は型安全マクロ (`sqlx::query!` / `query_as!` /
        `query_scalar!`) に統一し、オフラインビルド用 `.sqlx/`
        (`cargo sqlx prepare`) を整備する → 例外は FTS5 `MATCH` と 3 文字未満の
        LIKE フォールバックのみ (理由コメント +
        単体テスト付き)。`cargo sqlx prepare --workspace --check` と
        `SQLX_OFFLINE=true` / `DATABASE_URL` 未設定の双方でビルド可能
  - [x] `fxg-db`: FTS5 外部コンテンツ同期トリガー (INSERT/DELETE/UPDATE) と
        `searchable_text` 生成ロジック（`TerminalOutput`
        等のバイナリ系除外）の実装 → 抽出ロジック変更時の再計算用に
        `regenerate_searchable_text` / `rebuild_fts_index` も実装
  - [x] `fxg-db`: Local Node (`node.db`) と Central Server (`server.db`)
        で同一のクライアント向けレスポンス型を返す共通クエリ層の実装 →
        `SessionSummary` / `ProjectSummary` / `NodeSummary` /
        `PermissionRequestEntry` / `SearchHit` / `AuditLogEntry` /
        `SessionEventBatch` を両ロールで共通返却 (node / hub の応答一致をテスト)
- **完了条件 / 検証**:
  - `cargo test` で設定ファイルパース・マイグレーション・イベント適用の冪等性・
    投影再構築とイベント適用結果の一致・水位ベース Outbox 抽出・FTS5
    全文検索（トリガー経由含む）・`ts-rs` 型生成がすべて通ること。
  - `SQLX_OFFLINE=true` での `cargo build`（`.sqlx/` 使用）と
    `cargo sqlx prepare --check` が通ること。
  - ✅ 検証済み (2026-09-30): `mise run check` (fmt:check / clippy `-D warnings`
    / `cargo test --workspace`) が通過。`fxg-protocol` 94 件 + `fxg-db` 25 件
    (単体 12 / 統合 13) のテストが成功し、`mise run sqlx:check` と
    `DATABASE_URL` 未設定時のフォールバックビルドも成功。
- **実装ログ / 進捗メモ**:
  - **コミット**: `chore(workspace)` → `feat(fxg-protocol)` →
    `fix(fxg-protocol)` → `feat(fxg-db)` → `chore(mise)` → `docs(plan)` の順で 1
    トピック 1 コミット。
  - **仕様拡張 (`SessionCreated.node_id`)**: ハブ側 `sessions`
    投影の生成源である `SessionCreated` payload に `node_id`
    を追加した。イベントは接続元ノードから 送られるため transport
    上は暗黙に決まるが、payload に含めることで
    **イベントログのみからハブ投影を完全再構築**できる (Resync / DB 再構築時の
    復元を保証)。`docs/02` §0.3 と `docs/03` §1 に反映済み。
  - **イベント適用の FK 順序**: `session_events.session_id -> sessions` の FK を
    満たすため、`SessionCreated` のみイベント追記の**前**に `projects` /
    `sessions` 行を upsert する (冪等)。バッチは `node_seq` 昇順で渡すため
    未知セッションの最初のイベントは必ず `node_seq = 1` になる。
  - **SQLite の型推論対策**: `TEXT PRIMARY KEY` は暗黙 NOT NULL にならず sqlx が
    nullable と推定するため、共通クエリ層の `query_as!` では `AS "col!"`
    (非NULL) / `AS "col: bool"` (INTEGER→bool) の型オーバーライドを 使用する。
  - **`now_ms` 依存の値**: `nodes.last_seen_at` / `projects.updated_at` 等は
    登録時刻で決まるため、node / hub
    の応答一致テストでは該当時刻を除外して比較する。
  - **ts-rs 出力**: `.cargo/config.toml` の `TS_RS_EXPORT_DIR` で
    `ui/src/lib/generated/` を指定し、`TS_RS_LARGE_INT = "number"` により
    `node_seq` / epoch ms を `number` として出力する (`cargo test`
    が生成テストを兼ねる)。
  - **UI ディレクトリ**: `ui/src/lib/generated/` のみ先行作成 (Phase 5 で
    SvelteKit を構築)。生成物はコミット対象。

---

## Phase 2: プロセス/PTY制御 (`fxg-pty`) と ローカルノード基盤 (`fxg-node`, `fxg-cli` 基礎)

- **目的**:
  エージェントを載せる土台として、Windowsプロセスツリー管理・PTY・Gitプロジェクト/Worktree解決・Shadow
  Gitスナップショット・ローカルIPC・ローカルセキュリティを完成させる。
- **参照ドキュメント**:
  - チャンク配信と永続化（ターン完了時のみ永続化）:
    [`docs/01-architecture-and-sync.md` §2.3](../01-architecture-and-sync.md)
  - 論理プロジェクト解決 & Git Worktree 管理:
    [`docs/01-architecture-and-sync.md` §4, §5](../01-architecture-and-sync.md),
    [`docs/05-cli-and-pwa-ui.md` §2.2, §2.3](../05-cli-and-pwa-ui.md)
  - セキュリティ基礎 (Loopbackバインド・Token・Host/Origin検証):
    [`docs/01-architecture-and-sync.md` §7.1〜§7.3](../01-architecture-and-sync.md),
    [`docs/03-protocol-and-api.md` §3.0](../03-protocol-and-api.md),
    [`docs/05-cli-and-pwa-ui.md` §4 Milestone 1](../05-cli-and-pwa-ui.md)
  - CLI ⇔ Daemon ローカルIPC (Named Pipe / UDS):
    [`docs/03-protocol-and-api.md` §4](../03-protocol-and-api.md)
  - Shadow Git Tree (`GIT_INDEX_FILE` + `git write-tree`):
    [`docs/04-agent-drivers-and-windows.md` §4.1](../04-agent-drivers-and-windows.md)
  - Windows Job Object / `which` (`PATHEXT`) / `dunce` / `PtySessionManager`:
    [`docs/04-agent-drivers-and-windows.md` §5.1〜§5.3](../04-agent-drivers-and-windows.md)
  - CLI コマンド仕様: [`docs/05-cli-and-pwa-ui.md` §1](../05-cli-and-pwa-ui.md)
- **タスクリスト**:
  - [x] `fxg-pty`: Windows Job Object (`WinJobGuard`
        による親終了時の孫プロセス確実Kill)、`which::which_in` (`PATHEXT`
        解決)、`dunce::canonicalize` ヘルパーの実装 → 完了
        (`proc.rs`)。`ProcessTreeGuard` (Windows: kill-on-close Job Object /
        Unix: no-op) と孫プロセス巻き込み Kill の検証テスト
        (`job_object_kills_grandchildren_on_guard_drop`, `cfg(windows)`) を実装
  - [x] `fxg-pty`: `portable-pty` を用いた ConPTY / Unix PTY
        双方向ストリーム管理 (`PtySessionManager`、非同期Read/Write、動的Resize)
        → 完了 (`pty.rs`)。専用リーダー/ライタースレッド + `broadcast` による
        `PtyEvent` 配信、`resize` / `kill` / `kill_all`
        (緊急キルスイッチ用)。ConPTY 読み書きテストは `cfg(windows)` で用意
  - [x] `fxg-node`: 論理プロジェクト解決 (`normalize_git_url`, `.fxg.toml`,
        フォールバック) と Git Worktree
        検出・作成・削除（`~/.flexagent/worktrees/{project}/{branch}`
        テンプレート解決、`.fxg.toml` の `copy_files` / `post_create`
        フック実行） → 完了 (`project.rs` / `worktree.rs`)
  - [x] `fxg-node`: Shadow Git Tree (`GIT_INDEX_FILE` =
        `~/.flexagent/snapshots/<session-id>.index` + `git write-tree`)
        によるターン単位スナップショット取得・復元基盤の実装（セッション単位インデックス分離による競合回避、サイズ上限時のスキップ）
        → 完了 (`snapshot.rs`)。セッション単位のインデックス分離でターン
        スナップショットを取得し、Revert 用の復元基盤を整備
  - [x] `fxg-node`:
        ストリーミングチャンクのオンメモリ即時ブロードキャスト（`LiveStreamDelta`）と、ターン完了時の完成イベントのみの
        `node.db` 永続化 → 完了 (`session.rs`
        のイベントバス)。ストリーミング途中は メモリ配信のみ、ターン区切り/500ms
        結合フラッシュで `node.db` へ永続化
  - [x] `fxg-node`: ローカルIPCサーバー（Windows Named Pipe
        `\\.\pipe\fxg-daemon-<username>` / Unix Domain Socket、Length-prefixed
        JSON） → 完了 (`daemon/ipc.rs`)。テストは `IpcClient` 経由で Unix /
        Named Pipe の両方を同一コードで検証する (`testutil::test_ipc_endpoint`)
  - [x] `fxg-node`: ローカルHTTP/WSサーバー (`127.0.0.1:7860`
        厳格バインド)、`~/.flexagent/auth_token` 生成・永続化、Axum
        セキュリティミドルウェア (`Bearer`/`fxg_session` Cookie認証、`Host`
        ヘッダ検証、WS `Origin` ヘッダ検証) → 完了 (`daemon/http.rs` /
        `daemon/auth.rs`)。未認証 401 / 不正 Host・ Origin 403 のテスト付き
        (auth_token は 0600 で保存)
  - [x] `fxg-cli`: 基本サブコマンド (`fxg daemon`,
        `fxg project info/list/link/scan`, `fxg worktree list/add/remove/prune`,
        `fxg ps`, `fxg auth token/rotate-token`, ローカル `fxg kill-all`)
        の実装。 コマンド定義は `usage-rs` の `#[derive(Cli)]` / `Args` /
        `Subcommands` で行い、補完・help・`usage` spec 出力を標準装備する →
        完了。すべてローカルIPC経由で `fxg daemon` と通信 (`fxg session list` は
        `fxg ps` の別名)。`cli_e2e.rs` で実バイナリ +
        実デーモンのエンドツーエンドテストを実施
  - [x] CI: Windows (`windows-latest`) での自動テスト整備（Job Object
        による孫プロセス巻き込み Kill / Named Pipe IPC / ConPTY 読み書き） →
        完了 (`.github/workflows/ci.yml`)。Linux / macOS / Windows の
        3プラットフォームでテストし、fmt / clippy / `.sqlx` 検証 / ts-rs
        型同期チェックもジョブ化
- **完了条件 / 検証**:
  - `fxg daemon` 起動後、IPC経由で `fxg project info` / `fxg worktree` /
    `fxg ps`
    が動作し、Windows環境で親プロセス終了時に子プロセスツリーが確実に終了すること（`windows-latest`
    CI で自動検証）。また未認証・不正 `Host`/`Origin` アクセスが `401`/`403`
    で拒否されること。
  - ✅ 検証済み (2026-09-30): `mise run check` (fmt:check / clippy `-D warnings`
    / `cargo test --workspace`) が通過。全 195 件 (`fxg-protocol` 98 /
    `fxg-node` 57 / `fxg-db` 27 / `fxg-pty` 7 / `fxg-cli` 6)
    が成功。`cli_e2e.rs` は実 `fxg daemon` を起動し IPC 経由で `project` /
    `worktree` / `ps` / `auth token` を検証。
  - ✅ Windows 向け型チェック:
    `cargo check --workspace --all-targets
    --target x86_64-pc-windows-msvc`
    が通過 (Linux/macOS 開発機からの クロスチェック。実挙動は `windows-latest`
    CI で検証)
- **実装ログ / 進捗メモ**:
  - **コミット**: `feat(fxg-pty)` → `feat(fxg-node)` (worktree/snapshot) →
    `feat(fxg-node,fxg-db)` (イベントバス) →
    `feat(fxg-protocol,fxg-db,fxg-node)` (IPCメソッド/カーソル拡張) →
    `feat(fxg-node)` (デーモン: IPC/HTTP/認証) → `feat(fxg-cli,fxg-node)`
    (IPC経由CLI) → `test(...)` → `chore(mise)` → `chore(ci)` → `docs(plan)`
    の順で 1 トピック 1 コミット。
  - **IPCメソッド拡張 (Phase 3 以降の前提)**: `IpcClientMessage` / `IpcResult`
    に project / worktree / auth / session 系のメソッドを追加し、 Phase 2
    では未実装のエージェント系 (`EnsureSession` 等) は `INVALID_STATE`
    を明示返却する。
  - **IPC テストのクロスプラットフォーム化**: テスト用エンドポイントを
    `testutil::test_ipc_endpoint` に集約 (Windows: 連番付き一意 Named Pipe /
    Unix: 一時ソケット)。ラウンドトリップは `IpcClient` を経由させることで Unix
    Domain Socket / Named Pipe の両実装を同一テストコードで検証する。
  - **Job Object の孫プロセス Kill 検証**: kill-on-close Job にルートを
    割当てた後に孫 (`ping`) を起動させ (Job メンバーの子は自動で Job 所属)、
    ガード Drop で孫が死ぬことを `tasklist` で検証する。
  - **CI 構成**: `jdx/mise-action` でツールチェインを固定。 `cargo:sqlx-cli`
    は重いため `MISE_DISABLE_TOOLS` で sqlx ジョブ以外では 導入をスキップする
    (テストはコミット済み `.sqlx` のオフラインモードで動作)。
  - **`mise run check` の直列化**: `depends` (並列) だと cargo のターゲット
    ディレクトリロックで相互待ちが発生するため、`{ task = ... }` による 直列実行
    (`fmt:check` → `lint` → `test`) へ変更した。
  - **CI 初回実行の検証結果 (2026-09-30)**: 初回 CI は fmt / ubuntu / macos が
    成功し、windows-latest のみ `fxg-node` の snapshot テスト 2 件
    (`snapshot_and_restore_roundtrip` / `ignored_files_are_untouched`)
    が失敗した。原因は Windows 既定の `core.autocrlf=true` により
    スナップショット復元時に LF が CRLF へ変換される**実バグ** (`git add` +
    `checkout-index` の EOL 変換)。シャドウ Git 操作へ
    `-c core.autocrlf=false -c core.eol=lf` を前置してバイト忠実な復元へ
    修正し、`core.autocrlf=true` をリポジトリ設定で模擬する回帰テスト
    (`restore_preserves_bytes_under_autocrlf_repo_config`) を追加した
    (修正なしでは LF→CRLF 変換で失敗することを確認済み)。 併せてテストタスクを
    `cargo test --workspace --no-fail-fast` とし、 CI
    で全クレートの失敗を一度に確認できるようにした。
  - **CI 2回目の検証結果 (2026-09-30)**: autocrlf 修正後、残る失敗は `fxg-pty`
    の `spawns_cmd_on_windows` のみとなった。Windows の ConPTY は
    子プロセスが終了しても出力パイプが EOF にならず、ConPTY 自体のクローズ
    (`master` の Drop) で初めて EOF になるため、「リーダーの EOF 待ち →
    セッション破棄」の順では `PtyEvent::Exit` が**永遠に発火しない**実バグ (kill
    / kill_all 後もセッションが残り続ける) を検出。終了検知を `wait()`
    専用スレッドへ分離し、子プロセス終了 → セッション破棄 (master Drop →
    ConPTY/PTY クローズ) → リーダー EOF → `Exit` 配信の順に 修正した
    (全出力の配信完了を保証しつつ、フェイルセーフのタイムアウト付き)。
  - **CI 3回目以降の検証結果 (2026-09-30)**: 上記修正後も ConPTY テストは
    失敗し、診断 (受信出力 = 起動時のカーソル位置照会 `ESC[6n` のみ /
    子プロセス生存 / Exit 未配信) から、ConPTY が
    `PSEUDOCONSOLE_INHERIT_CURSOR` により**応答が届くまで子プロセスの
    コンソール操作をブロックする**ことが判明。テストから応答 (`ESC[1;1R`)
    を送ると即座に出力と Exit が得られることを確認した上で、恒久修正として
    マネージャ側で照会へ応答し、照会シーケンスを出力から除去する
    `ConPtyStartupHandshake` を実装した (アプリ自身が発行する 2 回目以降の
    照会はフロントエンドが応答できるよう透過。チャンク境界での分断にも
    対応)。これによりヘッドレス実行 (CI / エージェント駆動) でも Windows の
    子プロセスが停止しない。併せて、セッション破棄をロック外で行う修正と
    「登録前に remove してしまう」競合の解消、Windows 専用コードの clippy
    警告 (workspace 全体の `-D warnings`) の解消も行った。
  - ✅ **CI 最終検証 (2026-09-30)**: 全ジョブ成功 (fmt / clippy /
    sqlx / ts-rs 型同期 / test (ubuntu・macos・**windows-latest**))。
    windows-latest では Job Object の孫プロセス Kill・Named Pipe IPC・
    ConPTY 読み書きが自動検証され、Phase 2 完了条件を満たした
    (run `36684225515` / commit `1739914`)。

---

## Phase 3: エージェントドライバ (`fxg-acp`) と CLI / TUI 対話実行 (ACP & OpenCode2)

- **目的**: ローカル単体で `fxg run <acp-agent>` および `fxg run opencode`
  が完全に動作し、会話・Diff・承認・Revert/Fork が `node.db`
  と連動する状態を完成させる。
- **参照ドキュメント**:
  - `AgentDriver` / `ActiveSessionHandle` トレイト:
    [`docs/04-agent-drivers-and-windows.md` §1](../04-agent-drivers-and-windows.md)
  - ACP Registry 自動取得 & `AcpDriver` (`agent-client-protocol`):
    [`docs/04-agent-drivers-and-windows.md` §2](../04-agent-drivers-and-windows.md)
  - `OpenCode2Driver` (Server Bridge + 純正TUI Attach / ACPモード):
    [`docs/04-agent-drivers-and-windows.md` §3](../04-agent-drivers-and-windows.md)
  - セッション Revert & Fork:
    [`docs/04-agent-drivers-and-windows.md` §4](../04-agent-drivers-and-windows.md)
  - CLI コマンド仕様: [`docs/05-cli-and-pwa-ui.md` §1](../05-cli-and-pwa-ui.md)
- **タスクリスト**:
  - [ ] `fxg-acp`: `AgentDriver` / `ActiveSessionHandle` トレイト定義
  - [ ] `fxg-acp`: ACP Registry (`registry.json` / `[agents.custom.*]`)
        の取得・キャッシュと `binary` / `npx` / `uvx` 配布形態ごとの起動解決
        (`fxg agents list/install/update/remove`)
  - [ ] `fxg-acp`: `agent-client-protocol` を用いた `AcpDriver` (`FxgAcpClient`)
        実装（`session_update`, `request_permission` 待機チャネル,
        `read_text_file`/`write_text_file` + Unified Diff 計算, `terminal_*` ⇔
        `fxg-pty` 連携）
  - [ ] `fxg-acp`: `OpenCode2Driver` 実装（`opencode2 serve`
        起動・ランダムパスワード注入・SSE `/event`
        購読・OpenAPI操作・`opencode2 run --attach` 連携および `opencode2 acp`
        モード）
  - [ ] `fxg-node`: `SessionManager` への両ドライバ統合、Shadow Git Tree
        を用いた Revert（`fxg session revert`）と Session
        Fork（`fxg session fork`、ネイティブAPIまたは履歴Replay注入）の実装。`snapshot_tree_hash`
        はターン開始前の `UserMessage` イベント payload に含めて保存する
  - [ ] `fxg-node`: コマンド冪等性 (`command_id` 重複排除 → `COMMAND_DUPLICATE`)
        と busy 時 `SendPrompt` の Pending Queue 実装
  - [ ] `fxg-cli`: `ratatui` による内蔵TUI (`AcpTui`
        モード)、`NativeOpenCodeAttach`
        モード、`fxg run <agent>`、`fxg attach [session-id]`、`fxg session show/prompt/stop/kill/revert/fork`、`fxg inbox list/approve/reject`
        の実装（`usage-rs` の `RunWith` による async
        コマンドディスパッチを使用）
- **完了条件 / 検証**:
  - ターミナルから `fxg run <acp-agent>` および `fxg run opencode`
    を起動して対話・ツール承認・ファイル変更Diff記録・Revert/Fork
    が動作し、すべて `node.db` に記録されること。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）

---

## Phase 4: 中央サーバー (`fxg-server`) & Outbox 同期・Client API 共通化・LANセキュリティ

- **目的**: 常駐ノード (`fxg daemon`) と中央サーバー (`fxg server`)
  を接続し、Store-and-Forward 遅延同期と「Local Node / Central Server で同一の
  Client REST/WS API」を完成させる。
- **参照ドキュメント**:
  - Outbox 同期フロー & リモート操作ルーティング & 承認競合防止 &
    同期鮮度/競合解決:
    [`docs/01-architecture-and-sync.md` §2.2, §2.4, §3, §3.1](../01-architecture-and-sync.md)
  - LANセキュリティ・Web PTY制限・キルスイッチ・監査ログ:
    [`docs/01-architecture-and-sync.md` §7.3〜§7.5](../01-architecture-and-sync.md)
  - Node ⇔ Server プロトコル & Client REST/WS/PTY API
    (エラーコード・承認API含む):
    [`docs/03-protocol-and-api.md` §2, §3](../03-protocol-and-api.md)
- **タスクリスト**:
  - [ ] Client API 共通化: `fxg-node` (`127.0.0.1:7860`) と `fxg-server`
        (`:8080`) で同一の REST API
        エンドポイント群（[`docs/03-protocol-and-api.md` §3.1](../03-protocol-and-api.md)）と
        Client WS (`/api/v1/client/ws`)・PTY WS (`/api/v1/pty/ws`)
        を提供する共通ルーター/ハンドラ設計
  - [ ] `fxg-server`: Node Hub (`/api/v1/node/ws`)、ノード個別 `node_token`
        認証（`token_hash` 照合 + `NodeHello.node_id`
        一致検証）、`fxg auth node-token issue/revoke/list`、`NodeHello` /
        `ResyncRequest`（欠落・遅延セッションの再送要求）処理、 `EventBatchPush`
        の冪等保存 (`cursor` 採番 + 投影更新) と `EventBatchAck`（水位
        `acked_up_to_node_seq`）返却
  - [ ] `fxg-node`: Outbox Sync Worker（Outbound WS
        接続・再接続、`synced_up_to_node_seq` より後のイベントのバッチ送信 ➔ ACK
        で水位更新、`ResyncRequest` への範囲再送、リアルタイム `LiveStreamDelta`
        配信）
  - [ ] コマンド応答: `CommandResult`（`command_id` / `ErrorCode`
        付き）の要求元クライアントへの相関返却、ノードオフライン時の即時
        `NODE_OFFLINE` 返却
  - [ ] 承認 API: `POST /api/v1/sessions/:id/permissions/:req_id/respond`
        の実装（全クライアント横断の冪等解決、2 回目以降は `ALREADY_RESOLVED`）
  - [ ] 監査ログ: ローカル直結操作の `node.db.audit_logs` 記録と
        `/api/v1/audit/logs` の Local Node 対応
  - [ ] 双方向コマンドルーティング: 中央サーバー経由での `StartSession`,
        `SendPrompt`, `RespondPermission`, `ControlSession`, `ManageWorktree`,
        `GetGitDiff`, `PtySpawn/Input/Resize/Kill`
        の中継と、承認リクエスト解決時のマルチクライアント即時同期
        (`PermissionResolved`)
  - [ ] セキュリティ & 統制: `audit_logs` 記録（server.db / node.db
        双方）、`allow_remote_pty` ポリシー判定、全ノード一括の緊急キルスイッチ
        (`POST /api/v1/system/kill-switch` ➔ `KillAllSessions`)
- **完了条件 / 検証**:
  - `fxg server` 停止中に `fxg daemon`
    で実行したセッションイベントが、`fxg server`
    起動・再接続時に欠落・重複なく同期されること（ノード 2 プロセス + サーバー 1
    プロセスの E2E
    テストで自動検証）。また中央サーバーAPI経由でセッション操作・承認（二重承認の冪等含む）・キルスイッチが機能すること。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）

---

## Phase 5: Web UI / Android PWA (`ui/`)・Web Push・単一バイナリ統合

- **目的**: デスクトップ・ローカルフォールバック (`localhost:7860`)・Android PWA
  共通のレスポンシブSPAを構築し、VAPID Web Push と `rust-embed`
  単一バイナリ配信を完成させる。
- **参照ドキュメント**:
  - フロントエンド技術スタック・画面構成・PWA/Web Push・`ITerminalAdapter`:
    [`docs/05-cli-and-pwa-ui.md` §3.1〜§3.4](../05-cli-and-pwa-ui.md)
  - バックグラウンド常駐化 (`fxg service`):
    [`docs/04-agent-drivers-and-windows.md` §5.4](../04-agent-drivers-and-windows.md)
- **タスクリスト**:
  - [ ] `ui/` 基盤構築: SvelteKit (`Svelte 5` Runes +
        `@sveltejs/adapter-static`) + TypeScript + Tailwind CSS v4 +
        `shadcn-svelte` (`bits-ui`, `vaul-svelte`) + `oxlint` / `oxfmt` /
        `svelte-check` / `Vitest`
  - [ ] 状態管理 & 認証UI: `ts-rs` 生成型のインポート、差分同期 WebSocket ストア
        (`*.svelte.ts`。接続先ストアの `cursor` を保存し
        `Subscribe { since_cursor }` で差分再開、`event_id` で
        upsert・`LiveStreamDelta` を `message_id`
        でマージ)、初回トークン入力ダイアログ (`fxg_session`
        Cookie保持)、接続先スイッチャー（中央サーバー ⇔
        ローカルノード）、ヘッダー緊急停止（キルスイッチ）ボタン、同期状態バッジ（未同期件数
        / 最終同期時刻）
  - [ ] 主要画面実装:
    - [ ] グローバル承認 Inbox 画面（コマンド/Diffプレビュー、Approve / Allow
          Always / Reject ワンタップ応答）
    - [ ] プロジェクト & Worktree
          一覧画面（Worktree状態表示、新規Worktree作成、新規セッション起動、Context
          Fork）
    - [ ] セッション詳細画面（Chatペイン、思考折りたたみ、動的コントロールバー、スラッシュコマンド補完、`LiveStreamDelta`
          → 完成イベントの確定置換）
    - [ ] 2段階 Diff ペイン（セッション変更 ⇔ Worktree `vs Base` / `vs HEAD`
          切替、PC: `monaco-editor` / モバイル: `shiki` Unified Diff）
    - [ ] Terminal ペイン（`ITerminalAdapter` インターフェース +
          `GhosttyWebAdapter` (`@coder/ghostty-web`) + `/api/v1/pty/ws` 直結 +
          モバイル仮想キーバー + `allow_remote_pty=false`
          時のフォールバック表示）
    - [ ] 監査ログ (Audit Log) 画面 & FTS5 全文検索UI
  - [ ] Android PWA & VAPID Web Push:
        `manifest.webmanifest`、`src/service-worker.ts`（`$service-worker` App
        Shellキャッシュ + Push通知バナーの `[Approve]` / `[Reject]`
        バックグラウンドAPI呼び出し →
        `POST /api/v1/sessions/:id/permissions/:req_id/respond`）、`fxg-server`
        側の VAPID Push 送信実装
  - [ ] 単一バイナリ統合 & OSサービス化: `rust-embed` による `ui/build` の `fxg`
        バイナリ組み込み、`fxg web` コマンド（`--server`
        時の初回トークン入力フロー含む）、`fxg service install/uninstall/start/stop/restart/status`（Windows
        タスクスケジューラ / systemd / launchd）
- **完了条件 / 検証**:
  - `oxlint`, `oxfmt`, `svelte-check`, `vitest` がすべて通り、単一バイナリ `fxg`
    から配信されるWeb UIでチャット・Diff・Web PTY・Web
    Push承認が一貫して動作すること。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）

---

## Phase 6: 一時VM・サンドボックスノード (`fxg daemon --stdio` & Zero-Touch Provisioner)

- **目的**: VPNやポート開放に一切依存せず、Docker / Incus / Google Colab Pro
  等を子プロセスとしてスポーンし、`stdin/stdout`
  パイプ直結でステートレス実行・自動ツール構築・Gitバンドル退避までを完結させる。
- **参照ドキュメント**:
  - 一時VMアーキテクチャ・プロビジョナー設定（中央サーバーホスト上で起動）・`fxg bootstrap-workspace`・Bootstrap
    短命トークン認証・Git Credential Proxy・Graceful Drain（ベストエフォート）:
    [`docs/01-architecture-and-sync.md` §6.1〜§6.4](../01-architecture-and-sync.md),
    [`docs/05-cli-and-pwa-ui.md` §2.2, §2.3](../05-cli-and-pwa-ui.md)
  - Stdio トランスポート & メッセージ (`GitCredentialRequest`,
    `WorkspaceBundleUpload`, `DrainAndShutdown`):
    [`docs/03-protocol-and-api.md` §2](../03-protocol-and-api.md)
  - UI Bootstrap Log 表示 & CLI `--provisioner` 起動:
    [`docs/05-cli-and-pwa-ui.md` §1, §3.2, §4 Milestone 6](../05-cli-and-pwa-ui.md)
- **タスクリスト**:
  - [ ] `fxg-node`: `fxg daemon --stdio --ephemeral` 実装（`NodeToServerMsg` /
        `ServerToNodeMsg` を `stdin/stdout` JSON Lines
        で送受信するトランスポート実装、`stderr` へのログ完全分離）
  - [ ] `fxg-cli` / `fxg-node`: `fxg bootstrap-workspace` の実装（`mise` / `uv`
        単一バイナリ自動配置、`Cargo.toml` / `package.json` / `pyproject.toml` /
        `mise.toml` / `.fxg.toml` からのツール自動導入、出力の `>&2` 保護）
  - [ ] `fxg-server`:
        コマンドテンプレート型プロビジョナー管理（`~/.flexagent/config.toml` の
        `[provisioners.*]` からの子プロセス起動、`stderr` の `BootstrapLog`
        ストリーム配信、アイドルタイムアウト監視、プロビジョナー起動時の環境変数注入
        `FXG_GIT_URL` / `FXG_GIT_BRANCH` / 短命 `FXG_GIT_TOKEN`）
  - [ ] Git Credential Proxy: ブートストラップ中の `bootstrap-workspace` 内
        `GIT_ASKPASS` ヘルパー（短命トークン）と、デーモン起動後の
        `GitCredentialRequest` / `GitCredentialResponse`
        によるオンメモリ認証トークン中継
  - [ ] Graceful Drain & Git Bundle 退避/復元（ベストエフォート）: 破棄前の
        `DrainAndShutdown` 送信 ➔ 未送信イベント全フラッシュ +
        `git bundle create` (`WorkspaceBundleUpload`、サイズ上限・分割転送)
        による `server.db` (`git_bundle_path`)
        への保存、および別ノードでのバンドル復元
        (`restore_git_bundle_b64`)。Drain
        前クラッシュ時の中間イベント損失は許容仕様
  - [ ] CLI & UI 連携: `fxg run <agent> --provisioner <name>`,
        `fxg provisioners list/test`, UI
        新規セッション画面での一時VMプロビジョナー選択と Chat
        ペインの「Environment Bootstrap Log」折りたたみカード表示
- **完了条件 / 検証**:
  - 中央サーバー経由（`fxg run --provisioner` / Web
    UI）で一時コンテナ/VMが起動し、`stderr`
    のブートストラップログがUIにリアルタイム表示された後、`stdout` JSON Lines
    でセッションが開始され、終了時に `git bundle`
    が中央サーバーへ退避・別ノードで復元できること。Drain
    前クラッシュ時はイベントが失われる（ベストエフォート）ことも動作確認項目に含める。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）
