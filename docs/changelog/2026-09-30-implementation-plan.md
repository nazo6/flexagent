# FlexAgent (`fxg`) 段階的実装プラン & 進捗記録

- **日付**: 2026-09-30
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-pty`, `fxg-acp`,
  `fxg-node`, `fxg-server`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**: `Cargo.toml`, `mise.toml`, `dprint.json`,
  `tombi.toml`, `ui/package.json`

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
- **フォーマッタは言語ごとに固定する (mise 管理)**:
  - Rust = `rustfmt` (`cargo fmt`) / Markdown = `dprint`
    (`dprint.json`。`textWrap: maintainAndWrap` で手動の折り返しを尊重し、
    80桁超の行のみ折り返す。`wrapCodeSpans: false` で
    インラインコードを行分割しない) / TOML = `tombi` (`tombi.toml`) /
    UI (TS) = `oxfmt` (Phase 5)
  - 編集後は `mise run fmt`、コミット前は `mise run fmt:check`
    (CI の fmt ジョブでも検証)。TOML の lint (`mise run lint:toml`。
    JSON Schema 検証) も `mise run check` に含まれる
- **コミットは適切なタイミングで行う**:
  - 「フェーズ内のタスク項目が1つ完了した」「1トピックの変更が
    fmt/lint/テストを通った」 時点で、1トピック1コミットでコミットする
  - コミット前に `mise run fmt:check` と該当チェック（`cargo clippy` /
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
| **Phase 3** | エージェントドライバ (ACP / OpenCode2)・CLI/TUI・Revert/Fork | `fxg-acp`, `fxg-node`, `fxg-cli`          | 完了 (2026-09-30) |
| **Phase 4** | 中央サーバー・Outbox同期・Client API共通化・LANセキュリティ  | `fxg-server`, `fxg-node`, `fxg-cli`       | 完了 (2026-10-01) |
| **Phase 5** | Web UI / Android PWA・Web Push・単一バイナリ統合             | `ui`, `fxg-server`, `fxg-node`, `fxg-cli` | 完了 (2026-10-01) |
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
- **2026-09-30 (フォーマッタ決定)**:
  - Markdown = **dprint** (`dprint-plugin-markdown`。`textWrap:
    maintainAndWrap` で手動折り返しを尊重、`wrapCodeSpans: false` で
    インラインコードの行分割を防止)、TOML = **tombi** (formatter +
    linter + JSON Schema 検証)、Rust = rustfmt (既存)、UI の TS = `oxfmt`
    (Phase 5 の従来方針どおり)
  - `mise.toml` に `dprint` / `tombi` を固定し、タスクを `fmt:rust` /
    `fmt:md` / `fmt:toml` (集約 `fmt` / `fmt:check`) と `lint:rust` /
    `lint:toml` へ再構成。VS Code は `.vscode/settings.json` /
    `extensions.json` で dprint / tombi / rust-analyzer を推奨
  - 選定理由: 既存の整形挙動 (dprint 既定) と一致し、Node 依存を増やさない
    (TS は oxfmt 予定)。tombi は Cargo.toml 等のスキーマ検証も兼ねる
- **2026-10-01 (Phase 5 実装時の設計判断)**:
  - **認証フロー**: `POST /api/v1/auth/login` / `logout` を追加 (login 自体も
    共通ミドルウェアの Bearer 認証を要求し、body のトークンを再検証して
    `fxg_session` Cookie を発行)。Service Worker は Cookie 認証で Push
    バナーからの承認 API を呼ぶ
  - **接続先スイッチャーはオリジン単位のナビゲーション**:
    クロスオリジン fetch は Host/Origin 検証と CORS の双方で拒否されるため、
    登録済み URL へ `location.assign` で移動する方式とする (トークン/Cookie は
    オリジンごとに独立、登録先は localStorage に保存)
  - **UI の `$derived` はコンポーネント側で構成**: view
    より長生きするストア内の派生値は `derived_inert`
    で陳腐化するため、ストアは状態のみを保持し導出は消費側で行う
    (`SessionTimeline` は state のみ)
  - **Web Push は純 Rust 実装**: `web-push` 0.11 は `ece`
    経由で OpenSSL を要求し Windows のシステム依存が増えるため採用せず、
    `web-push-native` (p256 + aes-gcm + hkdf) + `reqwest`
    で暗号化・署名・送信を行う。VAPID 鍵は `~/.flexagent/vapid.json`
    に自動生成・永続化し、公開鍵を `GET /api/v1/system/info` で配布する
  - **埋め込み UI は認証外のフォールバック配信**: `rust-embed`
    (新クレート `fxg-ui-assets`) で `ui/build` を同梱し、共通ルーターの
    fallback として配信する (UI アセット自体は非機密。API は 401
    を返してトークンダイアログへ誘導)。`index.html` / `service-worker.js`
    は `no-cache`、ハッシュ付きアセットは長期キャッシュ
  - **Windows の `fxg service` フォールバック**: schtasks (ONLOGON)
    が権限拒否された場合はスタートアップフォルダの VBS
    ランチャー (ウィンドウ非表示) へフォールバックする (docs/04 §5.4 反映済み)
- **2026-10-01 (Phase 5 認証フロー)**:
  `POST /api/v1/auth/login` / `POST /api/v1/auth/logout` を追加。login
  自体も共通ミドルウェアの認証 (`Authorization: Bearer`) を要求し、body の
  トークンを再検証したうえで `fxg_session` Cookie
  (HttpOnly; SameSite=Strict) を発行する。PWA の Service Worker は Cookie
  認証で Push バナーからの承認 API を呼ぶ (docs/03 §3.0, §3.1 反映済み)
- **2026-10-01 (Phase 4 実装時の設計判断)**:
  - **共通 Client API レイヤの配置**: `fxg-server::api` に
    `ClientApiBackend` trait + 汎用ルーター/WSループを置き、`fxg-node` が
    依存して実装する。逆方向 (server → node) はセッション/エージェント機構への
    不要な逆依存になるため採用しない
  - **`NodeToServerMsg::WorktreeResult` 追加**: Worktree 作成の実結果 (実パス)
    を中央サーバーの REST 応答 (`WorktreeInfo`) に反映するため、`CommandResult`
    とは別の応答型を追加 (docs/03 §2 反映済み)
  - **`NodeToServerMsg::PtyError` 追加**: `allow_remote_pty=false` のノードが
    リモート `PtySpawn` を拒否したことをクライアント PTY WS へ非同期に伝える
    (`PtyServerMessage::Error` へ変換)。docs/03 §2 反映済み
  - **中央サーバー経由の Context Fork は Phase 6**: `fork_context_messages` /
    `restore_git_bundle_b64` は一時VM の Replay 注入・バンドル復元と同時に
    実装する (Phase 4 では `INVALID_STATE` で明示的に拒否)
  - **Web Push 購読 API はスタブ**: `POST /api/v1/push/subscribe` はルートのみ
    用意し `INVALID_STATE` を返す (VAPID 送信は Phase 5)

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
  - [x] `fxg-acp`: `AgentDriver` / `ActiveSessionHandle` トレイト定義
  - [x] `fxg-acp`: ACP Registry (`registry.json` / `[agents.custom.*]`)
        の取得・キャッシュと `binary` / `npx` / `uvx` 配布形態ごとの起動解決
        (`fxg agents list/install/update/remove` CLI も実装済み)
  - [x] `fxg-acp`: `agent-client-protocol` を用いた `AcpDriver` (`FxgAcpClient`)
        実装（`session_update`, `request_permission` 待機チャネル,
        `read_text_file`/`write_text_file` + Unified Diff 計算, `terminal_*` ⇔
        `fxg-pty` 連携）
  - [x] `fxg-acp`: `OpenCode2Driver` 実装（`opencode2 serve`
        起動・ランダムパスワード注入・SSE `/event`
        購読・OpenAPI操作・`opencode2 run --attach` 連携および `opencode2 acp`
        モード）
  - [x] `fxg-node`: `SessionManager` へのドライバ統合 (ACP)、Shadow Git Tree
        を用いた Revert（`fxg session revert`）と Session
        Fork（`fxg session fork`、ネイティブAPIまたは履歴Replay注入）の実装。`snapshot_tree_hash`
        はターン開始前の `UserMessage` イベント payload に含めて保存する
        （OpenCode2 ブリッジも `default_driver_factory` から接続済み）
  - [x] `fxg-node`: コマンド冪等性 (`command_id` 重複排除 → `COMMAND_DUPLICATE`)
        と busy 時 `SendPrompt` の Pending Queue 実装
  - [x] `fxg-cli`: `ratatui` による内蔵TUI (`AcpTui`
        モード。トランスクリプト描画・承認ダイアログ・スクロール・Ctrl+C
        デタッチ)、`NativeOpenCodeAttach`
        モード、`fxg run <agent>`、`fxg attach [session-id]`、`fxg session show/prompt/stop/kill/revert/fork`、`fxg inbox list/approve/reject`、`fxg agents list/install/update/remove`
        の実装（`usage-rs` の `RunAsync` による async
        コマンドディスパッチを使用）
- **完了条件 / 検証**:
  - ターミナルから `fxg run <acp-agent>` および `fxg run opencode`
    を起動して対話・ツール承認・ファイル変更Diff記録・Revert/Fork
    が動作し、すべて `node.db` に記録されること。
  - **検証状況**: `cli_e2e.rs` で IPC 往復（`EnsureSession` → セッション記録 →
    `ps` / `session show` / `inbox` / エラーコード）を自動検証済み
    (`mise run check` = fmt:check + clippy + test がすべて通過)。
    実エージェント (`opencode2`) との対話も手動検証済み (2026-09-30)。
    `fxg run opencode -d` でセッションを起動し、Client WS
    (`/api/v1/client/ws`) 経由で `capabilities_updated` の再生 →
    `SetConfig(model)`
    → `SendPrompt` を実行して実ターン (AgentMessage 完了) が `node.db`
    に記録されることを確認。プロセス強制終了 → デーモン再起動時の
    `Stopped` 整合と、`session kill` 後の孫プロセス残存なし (Windows Job
    Object) も確認済み。対話的TUI (`fxg run opencode` のアタッチ画面) と
    `antigravity` (ACP Registry) は手動確認を残す。
- **実装ログ / 進捗メモ (Phase 3)**:
  - **コミット**: `feat(fxg-acp)` (トレイト + Registry) → `feat(fxg-acp)`
    (AcpDriver) → `feat(fxg-node)` (SessionManager 統合 / Revert・Fork /
    冪等性) → `feat(fxg-acp)` (OpenCode2Driver) → `feat(fxg-cli)` (CLI / TUI)。
    以降も 1 トピック 1 コミットで進める。
  - **ACP SDK の採用 API**: `agent-client-protocol` 2.2.0 は旧 0.x 系の
    「Client トレイト実装」ではなく、`Client.builder()` +
    `on_receive_request/notification` + `ActiveSession` (build_session_from →
    start_session) というハンドラ/セッション API を採用している。
    `ConnectionTo<Agent>` は Clone 可能で、接続タスクが所有しつつ外部から
    cancel/set_mode/set_config を発行できる (docs/04 §1 のトレイトとは
    シグネチャが異なるため `mpsc::UnboundedSender` でイベントを送る形に調整)。
  - **エージェントプロセスの終了**: SDK に明示的な kill API はなく、
    接続 future の drop (または closure return) で `AcpAgent` の ChildGuard が
    プロセスグループごと SIGKILL する (Unix) / JT (Windows は
    CREATE_NO_WINDOW)。
    `shutdown()` はコマンドチャネル経由で closure を抜ける。
  - **承認のキャンセル**: ACP 仕様の MUST に従い、`session/cancel` 発行時に
    未解決の `request_permission` はすべて `cancelled` で応答する。
  - **SessionManager 統合 (完了)**: `default_driver_factory` による ACP ドライバ
    起動、`DriverEvent` → `SessionEventBus` のイベントポンプ、busy 管理を行い、
    IPC (`daemon/ipc.rs`) から `SessionEnsure` / `SessionControl` /
    `SessionPrompt` / `PermissionRespond` / `SessionRevert` / `SessionFork`
    をディスパッチする。
  - **Revert / Fork (完了)**: Revert は `UserMessage.snapshot_tree_hash`
    へファイルを復元し `SessionReverted`
    を追記（復元直前は `backup_tree_hash` へ退避、busy 中は `Busy`
    エラーで拒否）。Fork は履歴 Replay を 1
    プロンプトとして新セッションへ注入する
    (`parent_session_id` / `fork_from_node_seq` 記録、上限 16k 文字)。
  - **コマンド冪等性 / Pending Queue (完了)**: 直近 256 件の `command_id`
    を保持し重複を `CommandDuplicate` で拒否。busy 中の `SendPrompt`
    はキューに積み、ターン終了 (`idle`) 時に自動送信する。
  - **Windows Named Pipe の安定化**: 接続と接続の合間に listening
    インスタンスが消えると `ERROR_PIPE_BUSY` (231)
    が返るため、サーバー側は次インスタンスを
    先に作成してから接続を待ち、クライアント側は短いリトライで吸収する。
  - **OpenCode2Driver (完了)**: `opencode2 serve`
    ブリッジ（ランダムパスワード注入 + Basic 認証、SSE `/api/event`
    購読、`permission.*` ⇔ `RespondPermission`、ネイティブ Revert API）と
    純正TUI Attach (`opencode2 run --server <url> --session <id>`)。`--acp`
    指定時は `opencode2 acp` を標準ACPエージェントとして起動する。
  - **CLI / TUI (完了)**: `fxg run` / `fxg attach`（ID
    省略時はカレントディレクトリの直近アクティブセッションへ自動接続）/
    `fxg session show|prompt|stop|kill|revert|fork`（`prompt --wait`
    はターン完了までイベントを出力）/ `fxg inbox list|approve|reject` /
    `fxg agents list|install|update|remove` を実装。内蔵TUI は `ratatui` +
    `crossterm` でイベントストリームを描画し、プロンプト送信・承認応答 (y/a/n)
    を同一 IPC 接続から行う。Fork は `-w/--worktree` で新規 Worktree
    へ分岐できる。
  - **IPC / プロトコル拡張**: `EnsureSession` に `initial_mode` (`--mode`) と
    `acp` (`--acp`)、`SessionFork` に `cwd` (Worktree 分岐) を追加し、
    `SessionShow` / `InboxList` / 件数付き `SessionReverted` を追加した。
  - **テスト**: `cli_e2e.rs` に Phase 3 コマンドの IPC 往復テスト
    (未知セッションのエラーコード・承認 Inbox・起動失敗時のセッション記録 +
    `session show` 表示) を追加。並列実行時の Named Pipe 名衝突も修正した。
  - **手動検証で発見した不具合の修正 (2026-09-30)**:
    - **SSE の30秒タイムアウト切断**: `opencode2 serve` のイベント購読
      (`GET /api/event`) に `REQUEST_TIMEOUT` (30 秒) 付きの共有 HTTP
      クライアントを使っていたため、30 秒ごとにストリームが強制切断され
      イベントを取りこぼし、再接続上限 (3 回) 到達後は恒久的にイベントが
      届かなくなっていた。SSE 専用クライアント (`connect_timeout` のみ) に
      分離して修正。38 秒アイドル後もターンが完了することを実機確認した。
    - **起動直後の空カタログ**: `opencode2 serve` は起動直後しばらく
      `/api/agent` / `/api/model` が空配列を返すため、`fetch_capabilities`
      が `None` を返して `capabilities_updated` (モデル選択肢) が一切
      記録されなかった。内容が揃うまで 250ms 間隔で再取得する
      `wait_for_capabilities` を追加 (上限 10 秒)。実 opencode2
      に対する統合テストに「モデル選択肢付き capabilities が届く」検証を追加。
    - **ドライバ終了時の `Stopped` 記録**: イベントチャネルが閉じた
      (エージェントプロセス終了) 時点で `StatusChanged(Stopped)` を記録し
      active 一覧から外すようにした (`fxg ps` にゴーストセッションが
      残らない)。
    - **デーモン再起動時の整合**: デーモン強制終了で `idle` 等のまま残った
      セッションを起動時に `Stopped` として記録する
      (`reconcile_stale_sessions`)。
    - **`CommandAccepted` の扱い**: 非同期コマンド (`SendPrompt` /
      `RespondPermission` / `ControlSession`) の受理応答を CLI が失敗と
      誤判定していたため、成功として扱うよう修正。
    - **`ServerProcessGuard.terminate`**: `.cmd` シム経由の孫プロセス
      (実サーバー) を Job Object で確実に終了させるため、ジョブを明示的に
      閉じる `terminate` を追加 (`shutdown` 時 / サーバー自然終了時の両方)。

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
  - [x] Client API 共通化: `fxg-node` (`127.0.0.1:7860`) と `fxg-server`
        (`:8080`) で同一の REST API
        エンドポイント群（[`docs/03-protocol-and-api.md` §3.1](../03-protocol-and-api.md)）と
        Client WS (`/api/v1/client/ws`)・PTY WS (`/api/v1/pty/ws`)
        を提供する共通ルーター/ハンドラ設計 →
        完了。共通レイヤは `fxg-server::api` (ClientApiBackend trait + 汎用
        axum ルーター + セキュリティmiddleware + Client/PTY WS ループ)
        に集約し、
        `fxg-node` が依存する形で実装 (中央サーバー側の投影・中継との差分は
        トレイト実装に閉じ込め)
  - [x] `fxg-server`: Node Hub (`/api/v1/node/ws`)、ノード個別 `node_token`
        認証（`token_hash` 照合 + `NodeHello.node_id`
        一致検証）、`fxg auth node-token issue/revoke/list`、`NodeHello` /
        `ResyncRequest`（欠落・遅延セッションの再送要求）処理、 `EventBatchPush`
        の冪等保存 (`cursor` 採番 + 投影更新) と `EventBatchAck`（水位
        `acked_up_to_node_seq`）返却 → 完了
  - [x] `fxg-node`: Outbox Sync Worker（Outbound WS
        接続・再接続、`synced_up_to_node_seq` より後のイベントのバッチ送信 ➔ ACK
        で水位更新、`ResyncRequest` への範囲再送、リアルタイム `LiveStreamDelta`
        配信） → 完了 (指数バックオフ再接続、Worktree 変更時の NodeHello
        再送含む)
  - [x] コマンド応答: `CommandResult`（`command_id` / `ErrorCode`
        付き）の要求元クライアントへの相関返却、ノードオフライン時の即時
        `NODE_OFFLINE` 返却 → 完了 (Hub の pending 相関 + E2E で検証)
  - [x] 承認 API: `POST /api/v1/sessions/:id/permissions/:req_id/respond`
        の実装（全クライアント横断の冪等解決、2 回目以降は `ALREADY_RESOLVED`）
        →
        完了 (`always: true` の `allow_always` 昇格も実装。E2E で二重応答検証)
  - [x] 監査ログ: ローカル直結操作の `node.db.audit_logs` 記録と
        `/api/v1/audit/logs` の Local Node 対応 →
        完了 (session_start / permission_resolved / worktree_manage /
        pty_spawn / kill_switch を双方のDBへ記録)
  - [x] 双方向コマンドルーティング: 中央サーバー経由での `StartSession`,
        `SendPrompt`, `RespondPermission`, `ControlSession`, `ManageWorktree`,
        `GetGitDiff`, `PtySpawn/Input/Resize/Kill`
        の中継と、承認リクエスト解決時のマルチクライアント即時同期
        (`PermissionResolved`) → 完了 (`WorktreeResult` 応答を追加)
  - [x] セキュリティ & 統制: `audit_logs` 記録（server.db / node.db
        双方）、`allow_remote_pty` ポリシー判定、全ノード一括の緊急キルスイッチ
        (`POST /api/v1/system/kill-switch` ➔ `KillAllSessions`) → 完了
- **完了条件 / 検証**:
  - `fxg server` 停止中に `fxg daemon`
    で実行したセッションイベントが、`fxg server`
    起動・再接続時に欠落・重複なく同期されること（ノード 2 プロセス + サーバー 1
    プロセスの E2E
    テストで自動検証）。また中央サーバーAPI経由でセッション操作・承認（二重承認の冪等含む）・キルスイッチが機能すること。
    →
    **検証済み**。`crates/fxg-node/tests/sync_e2e.rs` にて
    (1) オフライン蓄積→再接続同期 (2 ノード・欠落/重複なし・水位ACK) と
    (2) 中央サーバー経由の `StartSession` (REST・初期プロンプト) /
    `SendPrompt` (Client WS 相関) / 承認の二重応答冪等化 /
    キルスイッチ配信+監査ログ / ノード停止時 `NODE_OFFLINE` を自動検証
- **実装ログ / 進捗メモ**:
  - 2026-10-01: Phase 4 完了。主な実装単位 (コミット順):
    1. `feat(fxg-server)`: 共通 Client API レイヤ
       (`ClientApiBackend` trait + 汎用ルーター + Host/Origin/Token
       middleware + Client WS / PTY WS) と `fxg-node` 側の実装移行
       (DaemonState がバックエンド実装。`SessionManager::start_session` /
       `git::workspace_diff` / `daemon::ops` (Worktree・監査の共通化) を追加)
    2. `feat(fxg-server)`: Node Hub (トークン認証・NodeHello/Resync・
       EventBatchPush 冪等適用 + ACK + Client WS 配信・コマンド相関中継
       (CommandResult/GitDiffResult/WorktreeResult)・PTY 中継・切断時
       NODE_OFFLINE・kill-switch ブロードキャスト)
    3. `feat(fxg-node)`: Outbox Sync Worker (再接続・水位フラッシュ・
       Resync 再送・リモートコマンド実行・`allow_remote_pty` ゲート・
       `central_connected` 状態)
    4. `feat(fxg-cli)`: `fxg server` / `fxg auth node-token issue|revoke|list`
       / `fxg daemon --server-url`
    5. `test(fxg-node)`: server + 2 nodes の E2E (`sync_e2e.rs`)
  - プロトコル追加: `NodeToServerMsg::WorktreeResult` (Worktree 作成の実結果を
    中央サーバー REST へ返す) と `NodeToServerMsg::PtyError`
    (リモートPTY無効/失敗の async 通知)。docs/03 に反映済み
  - Phase 6 送り (意図的): 中央サーバー経由の Context Fork
    (`fork_context_messages`
    / `restore_git_bundle_b64`)、プロビジョナー起動、Git Credential Proxy、
    `DrainAndShutdown`、Web Push (`POST /api/v1/push/subscribe` は
    `INVALID_STATE` を返すスタブ)
  - 2026-10-01: 実バイナリでのスモークテスト (`fxg server` と
    `fxg daemon --server-url` を別々の `FXG_HOME` で起動)
    を実施し、実プロセス間の
    ペアリング認証・`NodeHello` 接続 ("node connected")、ノード REST
    (`/api/v1/system/info` が `local_node` / `central_connected=true`)、
    サーバー REST (`/api/v1/nodes`
    のオンライン表示、`/api/v1/system/kill-switch`
    の配信、`/api/v1/audit/logs` の server/node 双方記録、未認証 401) を確認。
    この過程で `Server::wait` が起動直後に即時シャットダウンしてしまう回帰を
    発見・修正 (`fix(fxg-server)` + `server_lifecycle.rs` 回帰テストを追加)

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
  - [x] `ui/` 基盤構築: SvelteKit (`Svelte 5` Runes +
        `@sveltejs/adapter-static`) + TypeScript + Tailwind CSS v4 +
        `shadcn-svelte` (`bits-ui`, `vaul-svelte`) + `oxlint` / `oxfmt` /
        `svelte-check` / `Vitest` → 完了。SPA (`fallback: index.html`) +
        shadcn-svelte (Vega preset) + `mise` タスク (`fmt:ui` / `lint:ui` /
        `test:ui` / `build:ui` / `check:ui` / `ui:install`) と CI `ui` ジョブ
  - [x] 状態管理 & 認証UI: `ts-rs` 生成型のインポート、差分同期 WebSocket ストア
        (`*.svelte.ts`。接続先ストアの `cursor` を保存し
        `Subscribe { since_cursor }` で差分再開、`event_id` で
        upsert・`LiveStreamDelta` を `message_id`
        でマージ)、初回トークン入力ダイアログ (`fxg_session`
        Cookie保持)、接続先スイッチャー（中央サーバー ⇔
        ローカルノード）、ヘッダー緊急停止（キルスイッチ）ボタン、同期状態バッジ（未同期件数
        / 最終同期時刻） → 完了。`sync.svelte.ts` (指数バックオフ再接続 +
        Pending Queue + `CommandResult` 相関) / `connection.svelte.ts` /
        `reducer.ts` (純関数 + Vitest)
  - [x] 主要画面実装:
    - [x] グローバル承認 Inbox 画面（コマンド/Diffプレビュー、Approve / Allow
          Always / Reject ワンタップ応答） → 完了 (`PermissionCard` は
          Chat タイムラインと共用。`ALREADY_RESOLVED` は正常遷移)
    - [x] プロジェクト & Worktree
          一覧画面（Worktree状態表示、新規Worktree作成、新規セッション起動、Context
          Fork） → 完了 (`/projects` + `NewSessionDialog`。Context Fork は
          Phase 6 のサーバー側実装待ちのため UI にも露出しない)
    - [x] セッション詳細画面（Chatペイン、思考折りたたみ、動的コントロールバー、スラッシュコマンド補完、`LiveStreamDelta`
          → 完成イベントの確定置換） → 完了 (`/sessions/[id]` +
          Bootstrap Log カード + Composer (モード/設定/中断))
    - [x] 2段階 Diff ペイン（セッション変更 ⇔ Worktree `vs Base` / `vs HEAD`
          切替、PC: `monaco-editor` / モバイル: `shiki` Unified Diff） →
          完了 (`DiffPane` + `DiffViewer`。モバイル判定は 768px)
    - [x] Terminal ペイン（`ITerminalAdapter` インターフェース +
          `GhosttyWebAdapter` (`@coder/ghostty-web`) + `/api/v1/pty/ws` 直結 +
          モバイル仮想キーバー + `allow_remote_pty=false`
          時のフォールバック表示） → 完了。npm パッケージ名は
          `@coder/ghostty-web`
          ではなく `ghostty-web` (0.4.0) である点に注意
    - [x] 監査ログ (Audit Log) 画面 & FTS5 全文検索UI → 完了 (`/audit` /
          `/search`。FTS5 snippet の `[match]` マーカーを `<mark>` 化)
  - [x] Android PWA & VAPID Web Push:
        `manifest.webmanifest`、`src/service-worker.ts`（`$service-worker` App
        Shellキャッシュ + Push通知バナーの `[Approve]` / `[Reject]`
        バックグラウンドAPI呼び出し →
        `POST /api/v1/sessions/:id/permissions/:req_id/respond`）、`fxg-server`
        側の VAPID Push 送信実装 → 完了。鍵は `~/.flexagent/vapid.json`
        に自動生成·永続化し、`GET /api/v1/system/info` で公開鍵を配布。送信は
        `web-push-native` (純 Rust) + `reqwest` (`web-push` クレートは
        `ece` 経由で OpenSSL 必須のため不採用)
  - [x] 単一バイナリ統合 & OSサービス化: `rust-embed` による `ui/build` の `fxg`
        バイナリ組み込み、`fxg web` コマンド（`--server`
        時の初回トークン入力フロー含む）、`fxg service install/uninstall/start/stop/restart/status`（Windows
        タスクスケジューラ / systemd / launchd） → 完了。`fxg-ui-assets`
        クレート + 共通ルーターの SPA フォールバック (認証ミドルウェア外)。
        Windows は schtasks が拒否された場合にスタートアップフォルダ
        (VBS ランチャー / ウィンドウ非表示) へフォールバック
- **完了条件 / 検証**:
  - `oxlint`, `oxfmt`, `svelte-check`, `vitest` がすべて通り、単一バイナリ `fxg`
    から配信されるWeb UIでチャット・Diff・Web PTY・Web
    Push承認が一貫して動作すること。 → **検証済み** (2026-10-01)。
    実プロセス (`fxg daemon` + Vite dev / 埋め込みUI)
    でのブラウザスモークテスト:
    トークン入力→Cookie/WS 接続→プロジェクト/Worktree 表示→`opencode2`
    セッション起動→プロンプト送信 (ユーザーメッセージ + エラーノーティス表示)→
    監査ログ→Worktree vs HEAD Diff (shiki ハイライト)→ Web PTY spawn →
    `echo` 往復 (ghostty-web Canvas 描画)→ `?token=` 自動ログイン→
    埋め込み SPA 配信 (index.html / asset キャッシュヘッダ / 404 / 401) を確認。
    Windows での `fxg service` ライフサイクル (install → start → status →
    stop → uninstall) も確認
- **実装ログ / 進捗メモ**:
  - 2026-10-01: Phase 5 完了。主な実装単位 (コミット順):
    1. `feat(fxg-protocol,fxg-server,fxg-node)`: 認証 login/logout API
       (`fxg_session` Cookie 発行)
    2. `feat(ui)`: SvelteKit SPA + Tailwind v4 + shadcn-svelte + Oxc
       ツールチェイン
       (mise / CI 統合)
    3. `feat(ui)`: API クライアント + WS 差分同期ストア + タイムライン
       reducer (Vitest 付き)
    4. `feat(ui)`: アプリシェル (トークンダイアログ / 接続先切替 / キルスイッチ
       / 同期バッジ)
    5. `fix(ui)`: TokenDialog のマウント漏れと `derived_inert`
       (ストア内 `$derived` → コンポーネント側導出) の修正
    6. `feat(ui)`: 承認 Inbox + セッション詳細 (Chat / コントロールバー)
    7. `feat(ui)`: 2段階 Diff (Monaco / shiki)
    8. `feat(ui)`: プロジェクト & Worktree + 新規セッション
    9. `feat(ui)`: 監査ログ + 全文検索 + dev プロキシ
    10. `feat(ui)`: Web Terminal (ghostty-web + 仮想キーバー)
    11. `feat(fxg-db,fxg-server)`: VAPID Web Push (純 Rust)
    12. `feat(ui)`: PWA manifest / Service Worker / Push 購読
    13. `feat(fxg-cli,fxg-server)`: `rust-embed` 単一バイナリ + `fxg web` /
        `fxg service`
  - 設計判断 (Phase 5):
    - `POST /api/v1/auth/login` / `logout` を追加 (login 自体も Bearer
      認証必須、body のトークンを再検証)
    - 接続先スイッチャーは**オリジン単位のナビゲーション** (`location.assign`)。
      クロスオリジン fetch は Host/Origin 検証と CORS の双方で拒否されるため、
      同一オリジン厳格のまま運用する (トークン/Cookie もオリジンごとに独立)
    - UI の `$derived` はコンポーネント側で構成する。ストア (view
      より長生き) の `$derived` は `derived_inert` で陳腐化するため保持しない
    - Web Push は `web-push-native` (p256 + aes-gcm + hkdf) + `reqwest` で実装。
      `web-push` 0.11 は `ece` の OpenSSL バックエンドに依存し、Windows で
      システム OpenSSL を要求するため採用しない
    - 埋め込み UI は認証ミドルウェアの**外側**のフォールバックで配信する
      (UI 自体は非機密。API は 401 を返しトークンダイアログを誘導)
    - Windows の `fxg service` は schtasks (ONLOGON) を第一手段とし、
      権限拒否時はスタートアップフォルダの VBS ランチャーへフォールバック
  - 先送り (Phase 6 以降): Push 購読の明示削除 API
    (失効時は 404/410 で自動清除)、Context Fork の UI、一時VM
    プロビジョナー選択 UI、`fxg web --server` 時のトークン同梱
    (中央サーバー用トークンはローカルに無いため初回入力ダイアログに委ねる)
  - 2026-10-01 (追加改善): チャットの Markdown レンダリング。`marked` +
    `DOMPurify` (`ui/src/lib/markdown.ts`) でエージェントメッセージ /
    思考プロセスを常時 Markdown 描画する (`MarkdownText.svelte`。
    サニタイズ必須、リンクへ `target=_blank` / `rel=noopener` を付与)。
    コードフェンスは `$lib/highlight` の共有 shiki
    ハイライターを非同期適用する (未知言語はプレーンへフォールバック、内容
    キャッシュ付き。150ms デバウンスでストリーミング中の再描画に追従)。
    なお DOMPurify は happy-dom で正しく動作しない (未サポート環境)
    ため、`markdown.test.ts` のみ `// @vitest-environment jsdom` で実行する
    (jsdom を devDependency へ追加。実ブラウザでは問題なし)
  - 2026-10-01 (UIモダン化 & チャットUX改善):
    - **サイドバー型レイアウトへの移行 & プロジェクト別セッション一覧**:
      上部固定タブを廃止し、左側にセッション一覧とクイックナビゲーション（新規・Inbox・プロジェクト管理・検索・監査ログ）を集約した
      `AppSidebar.svelte` を導入。デスクトップは常駐、モバイルは `Sheet`
      ドロワーでスライドイン表示。セッション一覧は**プロジェクト別にアコーディオン形式でグループ化**（直近アクティビティ順ソート、未解決承認バッジ、プロジェクト単位の「＋」セッション起動ボタン付き）。最上部には緊急の要対応タスクを見落とさないよう
      `NEEDS ATTENTION`（承認待ち・エラー）セクションを配備。
    - **チャットUI型 新規セッション画面**:
      モーダルダイアログを廃止し、`/`（およびプロジェクト詳細からの遷移）をプロンプト入力ファーストの
      `NewSessionChat.svelte`
      に刷新。プロジェクト・ノード・エージェント・Worktree指定をインラインで切り替え、プロンプトを入力して
      Enter で即座にセッション作成＆チャット開始。
    - **複数ツールコールのコンパクト集約表示**: 連続するツール呼び出しを
      `ToolCallGroup.svelte`
      で1つの折りたたみアコーディオン（`Tool calls · N`）に集約。各ツールを1行サマリー（アイコン・コマンド/パス・成否）で表示し、クリックでDiffや標準出力をインライン展開。チャットログの見通しを大幅に向上。
    - **セッションノード稼働ステータス表示**: `NodeStatusBadge.svelte` および
      `node-status.ts`
      を追加。セッションヘッダー・Composer・サイドバーにノードのオンライン/オフライン/準備中（`ready`,
      `bootstrapping`, `offline`
      等）をリアルタイム表示し、オフライン時のプロンプト送信キューイング警告を明示。
    - **チャットフルハイト化 & デスクトップ分割表示**: 固定高 `max-h-[62svh]`
      を廃止してビューポート全高のスクロールコンテナ化（「最新へジャンプ」ボタン付き）。デスクトップ大画面ではチャットを見ながら右側に
      Diff や Terminal をサイドバイサイドで並行表示可能に。
    - **ACP / OpenCode2 セッションタイトル同期 & 初回プロンプト自動命名**:
      従来の `{エージェント名} @ {プロジェクト名}`
      一辺倒の固定タイトル設定を改修。ACP (`SessionUpdate::SessionInfoUpdate`)
      および `opencode2` (`session.updated`, `session.title.updated`)
      からエージェントが自律生成した `title` 通知を受信した際に
      `SessionTitleChanged` イベントを発行して DB / UI
      へリアルタイム反映する仕組みを接続。また、エージェント側がタイトル通知を行わない場合のフォールバックとして、セッション作成時の初期プロンプトまたは初回ターン送信時のプロンプトから先頭行（最大40文字、Markdown記号トリム、超過時
      `...` 付与）を抽出してタイトルを自動導出・即時更新するロジックを実装。
  - 2026-10-01 (CI 健全化 & Phase 1〜5 段階的修正着手):
    - **Stage 0 (CI 修正 - 完了)**:
      - Linux 環境での `fxg-cli` サービスステータス関数 (`service.rs`) を
        `async fn` 化し、`probe_running(server).await` に修正して Linux
        ビルドエラー (`E0277`, `E0308`) を解消。
      - `ui/package.json` の `"check"` スクリプトに `svelte-kit sync`
        を前置し、CI 環境での `.svelte-kit/tsconfig.json` 未生成による
        `svelte-check` 失敗を解消。
      - `fxg-node` (`session_manager.rs`) の Clippy 警告 (`too_many_arguments`
        を
        `StartDriverParams` 構造体に集約、`manual_pattern_char_comparison`
        を配列指定化、
        `collapsible_if` を `&& let` 化) を解消。
      - `fxg-cli` (`service.rs`) の `server_flag` を全プラットフォームの
        `status` ヒント表示で統一利用し、Linux CI での `dead_code` 警告を解消。
      - ✅ **CI 検証通過**: GitHub Actions CI (run `36801872999`) にて全 8
        ジョブ
        (fmt, clippy, sqlx, ts-rs, ui, test (ubuntu / macos / windows-latest))
        が完全成功。
    - **Stage 1 (規約準拠 & リソース管理 - 完了)**:
      - **Step 1.1**: `fxg-cli/src/commands.rs` の `same_directory`
        におけるパス正規化を `fxg_pty::canonicalize` (`dunce`) に統一し、Windows
        環境での UNC プレフィックス (`\\?\`) 付与を防止。
      - **Step 1.2**: `fxg-node/src/session_manager.rs` でセッション終了時
        (`pump_events` 完了および `shutdown_all`) に `remove_snapshot_index`
        を呼び出し、一時インデックスファイルの蓄積リークを解消。単体テスト
        `shutdown_all_cleans_up_snapshot_index` を追加。
      - **Step 1.3**: `fxg-pty/src/proc.rs` の `ProcessTreeGuard` に
        `attach_raw_handle` を追加し、`fxg-node/src/worktree.rs` の
        `run_hook_command` で子孫プロセスを Job Object にバインド＆60
        秒のタイムアウト (`tokio::time::timeout`) を適用。テストを追加。
      - ✅ **CI 検証通過**: GitHub Actions CI (run `36803865678`) にて全 8
        ジョブが完全成功。
    - **Stage 2 (Capabilities & エージェント連携基盤 - 完了)**:
      - **Step 2.1**: `fxg-acp/src/acp.rs` で `SessionCapabilitiesState`
        をメモリ保持し、差分更新通知 (`AvailableCommandsUpdate`,
        `CurrentModeUpdate`, `ConfigOptionUpdate`) 受信時に既存 capabilities
        をマージした完全な `CapabilitiesUpdated`
        を発行するよう改善。`fxg-db/src/events.rs` の `apply_projections`
        で空配列による上書きを防ぐ `COALESCE`
        防護を追加。`ui/src/lib/sync/reducer.ts` の `capabilitiesFromEvents`
        を逆順走査マージに改修。単体テスト追加。
      - **Step 2.2**: `fxg-acp/src/acp.rs` の `map_config_option` で
        `SessionConfigSelectOptions::Grouped`
        の選択肢をフラットに展開する処理を追加。単体テスト追加。
      - **Step 2.3**: `fxg-acp/src/acp.rs`
        で外部エージェントプロセスを起動する際、`ProcessTreeGuard` で Windows
        Job Object にバインドし、エージェント stderr
        をログポンプに流すとともに、親終了時・セッション終了時の孫プロセスを含めた確実な終了を保証。
      - ✅ **CI 検証通過**: GitHub Actions CI (run `36805517859`) にて全 8
        ジョブ (fmt, clippy, sqlx, ts-rs, ui, test (ubuntu / macos /
        windows-latest))
        が完全成功。
    - **Stage 3 (UI/UX & 同期堅牢化 - 未着手 / 残項目)**:
      - **Step 3.1**: OpenCode2 モデル選択の Composer UI サポート
        (`ui/src/lib/components/chat/Composer.svelte` でオブジェクト形式の
        config
        option `{ providerID, id, name }` をサポートし、JSON
        文字列化と復元を行い `set_config` を発行)。
      - **Step 3.2**: DB リセット時カーソル不整合防御
        (`ui/src/lib/stores/sync.svelte.ts` / サーバー・ノード WS で DB
        再作成・リセット時に cursor 不整合を検知して 0 または最新から再同期)。
      - **Step 3.3**: TerminalView のセッション切り替え時再接続
        (`ui/src/lib/components/terminal/TerminalView.svelte` で `$effect`
        による
        `sessionId` 監視と WS 再接続)。
    - **Stage 4 (スキーマ対称性の完全化 - 未着手 / 残項目)**:
      - **Step 4.1**: `sessions` テーブルへの `available_modes_json` 追加
        (SQLite マイグレーション、DB/Protocol/UI 型、および `.sqlx`
        クエリメタデータの完全同期)。

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
