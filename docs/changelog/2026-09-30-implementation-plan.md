# FlexAgent (`fxg`) 段階的実装プラン & 進捗記録

- **日付**: 2026-09-30
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-pty`, `fxg-acp`, `fxg-node`, `fxg-server`, `fxg-cli`, `ui`
- **対象スクリプト / 設定**: `Cargo.toml`, `mise.toml`, `ui/package.json`

---

## 運用ルール（初期実装期間中）

- 初期実装（Phase 1 〜 Phase 6）の期間中は、新しい changelog ファイルを作成せず、**本ファイル (`docs/changelog/2026-09-30-implementation-plan.md`) をマスタープラン兼進捗記録として参照・順次更新**すること。
- 各構造体・SQLスキーマ・プロトコル・UI仕様の詳細定義は本ファイルに重複記載せず、必ず以下の**元設計ドキュメント**を直接参照して実装すること：
  - [`docs/README.md`](../README.md): コアコンセプト・クレート構成
  - [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md): 全体トポロジー、Outbox同期、論理プロジェクト・Worktree解決、一時VM (`--stdio`)、セキュリティ
  - [`docs/02-database-schema.md`](../02-database-schema.md): `server.db` / `node.db` の SQLite スキーマ定義
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md): `fxg-protocol` 型定義、WS/Stdio/IPC メッセージ、REST API エンドポイント
  - [`docs/04-agent-drivers-and-windows.md`](../04-agent-drivers-and-windows.md): `AgentDriver`、ACP / `opencode2` 統合、Shadow Git Revert/Fork、Windows 固有実装
  - [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md): CLI コマンド完全リファレンス、設定ファイルスキーマ (`config.toml` / `.fxg.toml`)、PWA / Web Push 設計、ターミナル抽象化 (`ITerminalAdapter`)

---

## 実装フェーズ一覧

| フェーズ | 対象領域 | 主な対象パッケージ | 状態 |
| :--- | :--- | :--- | :--- |
| **Phase 1** | ワークスペース・共通プロトコル・設定スキーマ・DB基盤 | `fxg-protocol`, `fxg-db` | 未着手 |
| **Phase 2** | プロセス/PTY制御・Windows対応・ローカルノード基盤 | `fxg-pty`, `fxg-node`, `fxg-cli` | 未着手 |
| **Phase 3** | エージェントドライバ (ACP / OpenCode2)・CLI/TUI・Revert/Fork | `fxg-acp`, `fxg-node`, `fxg-cli` | 未着手 |
| **Phase 4** | 中央サーバー・Outbox同期・Client API共通化・LANセキュリティ | `fxg-server`, `fxg-node`, `fxg-cli` | 未着手 |
| **Phase 5** | Web UI / Android PWA・Web Push・単一バイナリ統合 | `ui`, `fxg-server`, `fxg-node`, `fxg-cli` | 未着手 |
| **Phase 6** | 一時VM・サンドボックスノード (`--stdio` & Zero-Touch構築) | `fxg-node`, `fxg-server`, `fxg-cli`, `ui` | 未着手 |

---

## Phase 1: ワークスペース構築・共通プロトコル (`fxg-protocol`)・DBレイヤー (`fxg-db`)

- **目的**: 全クレートとUIが参照する「型」「設定スキーマ」「SQLiteスキーマ」を最初に確定させ、後続フェーズでの型変換の書き直しや仮構造体の作成を排除する。
- **参照ドキュメント**:
  - クレート構成: [`docs/README.md` §リポジトリ・クレート構成](../README.md)
  - イベントID・採番設計: [`docs/01-architecture-and-sync.md` §2.1](../01-architecture-and-sync.md)
  - SQLite スキーマ (`server.db` / `node.db`)・FTS5: [`docs/02-database-schema.md` §1, §2](../02-database-schema.md)
  - プロトコル・通信メッセージ・IPC型 (`ts-rs`): [`docs/03-protocol-and-api.md` §1〜§4](../03-protocol-and-api.md)
  - 設定ファイルスキーマ (`config.toml` / `.fxg.toml`) & `~/.flexagent/` 構成: [`docs/05-cli-and-pwa-ui.md` §2.1〜§2.3](../05-cli-and-pwa-ui.md)
- **タスクリスト**:
  - [ ] Cargo Workspace (`Cargo.toml` 全7クレート構成) と `mise.toml`（ビルド・`ts-rs` 型出力・lint・format タスク）の初期構築
  - [ ] `fxg-protocol`: `SessionEventEnvelope` および `UnifiedEventPayload` の定義 (`ts-rs` derive 付与)
  - [ ] `fxg-protocol`: Node ⇔ Server メッセージ (`NodeToServerMsg`, `ServerToNodeMsg`) の定義
  - [ ] `fxg-protocol`: Client REST API リクエスト/レスポンス型、Client WS / PTY WS メッセージ型、Local IPC メッセージ型の定義
  - [ ] `fxg-protocol`: グローバル設定 (`~/.flexagent/config.toml` + `FXG_*` 環境変数オーバーライド) およびプロジェクト設定 (`.fxg.toml`) のデシリアライズ構造体定義
  - [ ] `fxg-db`: `node.db` (`local_sessions`, `local_session_events` + Outbox `synced = 0` インデックス) のマイグレーションとクエリ実装
  - [ ] `fxg-db`: `server.db` (`nodes`, `projects`, `project_node_bindings`, `sessions`, `session_events`, `session_events_fts` (`tokenize='trigram'`), `permission_requests`, `push_subscriptions`, `audit_logs`) のマイグレーションとクエリ実装
  - [ ] `fxg-db`: Local Node (`node.db`) と Central Server (`server.db`) で同一のクライアント向けレスポンス型を返す共通クエリ層の実装
- **完了条件 / 検証**:
  - `cargo test` で設定ファイルパース・マイグレーション・Outbox未送信抽出・冪等挿入 (`INSERT OR IGNORE`)・FTS5 全文検索・`ts-rs` 型生成がすべて通ること。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）

---

## Phase 2: プロセス/PTY制御 (`fxg-pty`) と ローカルノード基盤 (`fxg-node`, `fxg-cli` 基礎)

- **目的**: エージェントを載せる土台として、Windowsプロセスツリー管理・PTY・Gitプロジェクト/Worktree解決・Shadow Gitスナップショット・ローカルIPC・ローカルセキュリティを完成させる。
- **参照ドキュメント**:
  - チャンク Compaction (500ms / ターン区切り): [`docs/01-architecture-and-sync.md` §2.3](../01-architecture-and-sync.md)
  - 論理プロジェクト解決 & Git Worktree 管理: [`docs/01-architecture-and-sync.md` §4, §5](../01-architecture-and-sync.md), [`docs/05-cli-and-pwa-ui.md` §2.2, §2.3](../05-cli-and-pwa-ui.md)
  - セキュリティ基礎 (Loopbackバインド・Token・Host/Origin検証): [`docs/01-architecture-and-sync.md` §7.1〜§7.3](../01-architecture-and-sync.md), [`docs/03-protocol-and-api.md` §3.0](../03-protocol-and-api.md), [`docs/05-cli-and-pwa-ui.md` §4 Milestone 1](../05-cli-and-pwa-ui.md)
  - CLI ⇔ Daemon ローカルIPC (Named Pipe / UDS): [`docs/03-protocol-and-api.md` §4](../03-protocol-and-api.md)
  - Shadow Git Tree (`GIT_INDEX_FILE` + `git write-tree`): [`docs/04-agent-drivers-and-windows.md` §4.1](../04-agent-drivers-and-windows.md)
  - Windows Job Object / `which` (`PATHEXT`) / `dunce` / `PtySessionManager`: [`docs/04-agent-drivers-and-windows.md` §5.1〜§5.3](../04-agent-drivers-and-windows.md)
  - CLI コマンド仕様: [`docs/05-cli-and-pwa-ui.md` §1](../05-cli-and-pwa-ui.md)
- **タスクリスト**:
  - [ ] `fxg-pty`: Windows Job Object (`WinJobGuard` による親終了時の孫プロセス確実Kill)、`which::which_in` (`PATHEXT` 解決)、`dunce::canonicalize` ヘルパーの実装
  - [ ] `fxg-pty`: `portable-pty` を用いた ConPTY / Unix PTY 双方向ストリーム管理 (`PtySessionManager`、非同期Read/Write、動的Resize)
  - [ ] `fxg-node`: 論理プロジェクト解決 (`normalize_git_url`, `.fxg.toml`, フォールバック) と Git Worktree 検出・作成・削除（`~/.flexagent/worktrees/{project}/{branch}` テンプレート解決、`.fxg.toml` の `copy_files` / `post_create` フック実行）
  - [ ] `fxg-node`: Shadow Git Tree (`GIT_INDEX_FILE` + `git write-tree`) によるターン単位スナップショット取得・復元基盤の実装
  - [ ] `fxg-node`: ストリーミングチャンクのオンメモリ即時ブロードキャスト＋500ms/ターン区切りでの `node.db` 結合フラッシュ (Compaction)
  - [ ] `fxg-node`: ローカルIPCサーバー（Windows Named Pipe `\\.\pipe\fxg-daemon-<username>` / Unix Domain Socket、Length-prefixed JSON）
  - [ ] `fxg-node`: ローカルHTTP/WSサーバー (`127.0.0.1:7860` 厳格バインド)、`~/.flexagent/auth_token` 生成・永続化、Axum セキュリティミドルウェア (`Bearer`/`fxg_session` Cookie認証、`Host` ヘッダ検証、WS `Origin` ヘッダ検証)
  - [ ] `fxg-cli`: 基本サブコマンド (`fxg daemon`, `fxg project info/list/link/scan`, `fxg worktree list/add/remove/prune`, `fxg ps`, `fxg auth token/rotate-token/node-token`, ローカル `fxg kill-all`) の実装
- **完了条件 / 検証**:
  - `fxg daemon` 起動後、IPC経由で `fxg project info` / `fxg worktree` / `fxg ps` が動作し、Windows環境で親プロセス終了時に子プロセスツリーが確実に終了すること。また未認証・不正 `Host`/`Origin` アクセスが `401`/`403` で拒否されること。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）

---

## Phase 3: エージェントドライバ (`fxg-acp`) と CLI / TUI 対話実行 (ACP & OpenCode2)

- **目的**: ローカル単体で `fxg run <acp-agent>` および `fxg run opencode` が完全に動作し、会話・Diff・承認・Revert/Fork が `node.db` と連動する状態を完成させる。
- **参照ドキュメント**:
  - `AgentDriver` / `ActiveSessionHandle` トレイト: [`docs/04-agent-drivers-and-windows.md` §1](../04-agent-drivers-and-windows.md)
  - ACP Registry 自動取得 & `AcpDriver` (`agent-client-protocol`): [`docs/04-agent-drivers-and-windows.md` §2](../04-agent-drivers-and-windows.md)
  - `OpenCode2Driver` (Server Bridge + 純正TUI Attach / ACPモード): [`docs/04-agent-drivers-and-windows.md` §3](../04-agent-drivers-and-windows.md)
  - セッション Revert & Fork: [`docs/04-agent-drivers-and-windows.md` §4](../04-agent-drivers-and-windows.md)
  - CLI コマンド仕様: [`docs/05-cli-and-pwa-ui.md` §1](../05-cli-and-pwa-ui.md)
- **タスクリスト**:
  - [ ] `fxg-acp`: `AgentDriver` / `ActiveSessionHandle` トレイト定義
  - [ ] `fxg-acp`: ACP Registry (`registry.json` / `[agents.custom.*]`) の取得・キャッシュと `binary` / `npx` / `uvx` 配布形態ごとの起動解決 (`fxg agents list/install/update/remove`)
  - [ ] `fxg-acp`: `agent-client-protocol` を用いた `AcpDriver` (`FxgAcpClient`) 実装（`session_update`, `request_permission` 待機チャネル, `read_text_file`/`write_text_file` + Unified Diff 計算, `terminal_*` ⇔ `fxg-pty` 連携）
  - [ ] `fxg-acp`: `OpenCode2Driver` 実装（`opencode2 serve` 起動・ランダムパスワード注入・SSE `/event` 購読・OpenAPI操作・`opencode2 run --attach` 連携および `opencode2 acp` モード）
  - [ ] `fxg-node`: `SessionManager` への両ドライバ統合、Shadow Git Tree を用いた Revert（`fxg session revert`）と Session Fork（`fxg session fork`、ネイティブAPIまたは履歴Replay注入）の実装
  - [ ] `fxg-cli`: `ratatui` による内蔵TUI (`AcpTui` モード)、`NativeOpenCodeAttach` モード、`fxg run <agent>`、`fxg attach [session-id]`、`fxg session show/prompt/stop/kill/revert/fork`、`fxg inbox list/approve/reject` の実装
- **完了条件 / 検証**:
  - ターミナルから `fxg run <acp-agent>` および `fxg run opencode` を起動して対話・ツール承認・ファイル変更Diff記録・Revert/Fork が動作し、すべて `node.db` に記録されること。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）

---

## Phase 4: 中央サーバー (`fxg-server`) & Outbox 同期・Client API 共通化・LANセキュリティ

- **目的**: 常駐ノード (`fxg daemon`) と中央サーバー (`fxg server`) を接続し、Store-and-Forward 遅延同期と「Local Node / Central Server で同一の Client REST/WS API」を完成させる。
- **参照ドキュメント**:
  - Outbox 同期フロー & リモート操作ルーティング & 承認競合防止: [`docs/01-architecture-and-sync.md` §2.2, §3](../01-architecture-and-sync.md)
  - LANセキュリティ・Web PTY制限・キルスイッチ・監査ログ: [`docs/01-architecture-and-sync.md` §7.3〜§7.5](../01-architecture-and-sync.md)
  - Node ⇔ Server プロトコル & Client REST/WS/PTY API: [`docs/03-protocol-and-api.md` §2, §3](../03-protocol-and-api.md)
- **タスクリスト**:
  - [ ] Client API 共通化: `fxg-node` (`127.0.0.1:7860`) と `fxg-server` (`:8080`) で同一の REST API エンドポイント群（[`docs/03-protocol-and-api.md` §3.1](../03-protocol-and-api.md)）と Client WS (`/api/v1/client/ws`)・PTY WS (`/api/v1/pty/ws`) を提供する共通ルーター/ハンドラ設計
  - [ ] `fxg-server`: Node Hub (`/api/v1/node/ws`)、`node_token` 認証、`NodeHello` / `SessionUpsert` 処理、`EventBatchPush` の冪等保存 (`global_seq` 採番) と `EventBatchAck` 返却
  - [ ] `fxg-node`: Outbox Sync Worker（Outbound WS 接続・再接続、未同期 `synced = 0` イベントのバッチ送信、リアルタイム `LiveStreamDelta` 配信）
  - [ ] 双方向コマンドルーティング: 中央サーバー経由での `StartSession`, `SendPrompt`, `RespondPermission`, `ControlSession`, `ManageWorktree`, `GetGitDiff`, `PtySpawn/Input/Resize/Kill` の中継と、承認リクエスト解決時のマルチクライアント即時同期 (`PermissionResolved`)
  - [ ] セキュリティ & 統制: `audit_logs` 記録、`allow_remote_pty` ポリシー判定、全ノード一括の緊急キルスイッチ (`POST /api/v1/system/kill-switch` ➔ `KillAllSessions`)
- **完了条件 / 検証**:
  - `fxg server` 停止中に `fxg daemon` で実行したセッションイベントが、`fxg server` 起動・再接続時に欠落・重複なく同期されること。また中央サーバーAPI経由でセッション操作・承認・キルスイッチが機能すること。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）

---

## Phase 5: Web UI / Android PWA (`ui/`)・Web Push・単一バイナリ統合

- **目的**: デスクトップ・ローカルフォールバック (`localhost:7860`)・Android PWA 共通のレスポンシブSPAを構築し、VAPID Web Push と `rust-embed` 単一バイナリ配信を完成させる。
- **参照ドキュメント**:
  - フロントエンド技術スタック・画面構成・PWA/Web Push・`ITerminalAdapter`: [`docs/05-cli-and-pwa-ui.md` §3.1〜§3.4](../05-cli-and-pwa-ui.md)
  - バックグラウンド常駐化 (`fxg service`): [`docs/04-agent-drivers-and-windows.md` §5.4](../04-agent-drivers-and-windows.md)
- **タスクリスト**:
  - [ ] `ui/` 基盤構築: SvelteKit (`Svelte 5` Runes + `@sveltejs/adapter-static`) + TypeScript + Tailwind CSS v4 + `shadcn-svelte` (`bits-ui`, `vaul-svelte`) + `oxlint` / `oxfmt` / `svelte-check` / `Vitest`
  - [ ] 状態管理 & 認証UI: `ts-rs` 生成型のインポート、`last_global_seq` 差分同期 WebSocket ストア (`*.svelte.ts`)、初回トークン入力ダイアログ (`fxg_session` Cookie保持)、接続先スイッチャー（中央サーバー ⇔ ローカルノード）、ヘッダー緊急停止（キルスイッチ）ボタン
  - [ ] 主要画面実装:
    - [ ] グローバル承認 Inbox 画面（コマンド/Diffプレビュー、Approve / Allow Always / Reject ワンタップ応答）
    - [ ] プロジェクト & Worktree 一覧画面（Worktree状態表示、新規Worktree作成、新規セッション起動、Context Fork）
    - [ ] セッション詳細画面（Chatペイン、思考折りたたみ、動的コントロールバー、スラッシュコマンド補完）
    - [ ] 2段階 Diff ペイン（セッション変更 ⇔ Worktree `vs Base` / `vs HEAD` 切替、PC: `monaco-editor` / モバイル: `shiki` Unified Diff）
    - [ ] Terminal ペイン（`ITerminalAdapter` インターフェース + `GhosttyWebAdapter` (`@coder/ghostty-web`) + `/api/v1/pty/ws` 直結 + モバイル仮想キーバー + `allow_remote_pty=false` 時のフォールバック表示）
    - [ ] 監査ログ (Audit Log) 画面 & FTS5 全文検索UI
  - [ ] Android PWA & VAPID Web Push: `manifest.webmanifest`、`src/service-worker.ts`（`$service-worker` App Shellキャッシュ + Push通知バナーの `[Approve]` / `[Reject]` バックグラウンドAPI呼び出し）、`fxg-server` 側の VAPID Push 送信実装
  - [ ] 単一バイナリ統合 & OSサービス化: `rust-embed` による `ui/build` の `fxg` バイナリ組み込み、`fxg web` コマンド、`fxg service install/uninstall/start/stop/restart/status`（Windows タスクスケジューラ / systemd / launchd）
- **完了条件 / 検証**:
  - `oxlint`, `oxfmt`, `svelte-check`, `vitest` がすべて通り、単一バイナリ `fxg` から配信されるWeb UIでチャット・Diff・Web PTY・Web Push承認が一貫して動作すること。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）

---

## Phase 6: 一時VM・サンドボックスノード (`fxg daemon --stdio` & Zero-Touch Provisioner)

- **目的**: VPNやポート開放に一切依存せず、Docker / Incus / Google Colab Pro 等を子プロセスとしてスポーンし、`stdin/stdout` パイプ直結でステートレス実行・自動ツール構築・Gitバンドル退避までを完結させる。
- **参照ドキュメント**:
  - 一時VMアーキテクチャ・プロビジョナー設定・`fxg bootstrap-workspace`・Git Credential Proxy・Graceful Drain: [`docs/01-architecture-and-sync.md` §6.1〜§6.4](../01-architecture-and-sync.md), [`docs/05-cli-and-pwa-ui.md` §2.2, §2.3](../05-cli-and-pwa-ui.md)
  - Stdio トランスポート & メッセージ (`GitCredentialRequest`, `WorkspaceBundleUpload`, `DrainAndShutdown`): [`docs/03-protocol-and-api.md` §2](../03-protocol-and-api.md)
  - UI Bootstrap Log 表示 & CLI `--provisioner` 起動: [`docs/05-cli-and-pwa-ui.md` §1, §3.2, §4 Milestone 6](../05-cli-and-pwa-ui.md)
- **タスクリスト**:
  - [ ] `fxg-node`: `fxg daemon --stdio --ephemeral` 実装（`NodeToServerMsg` / `ServerToNodeMsg` を `stdin/stdout` JSON Lines で送受信するトランスポート実装、`stderr` へのログ完全分離）
  - [ ] `fxg-cli` / `fxg-node`: `fxg bootstrap-workspace` の実装（`mise` / `uv` 単一バイナリ自動配置、`Cargo.toml` / `package.json` / `pyproject.toml` / `mise.toml` / `.fxg.toml` からのツール自動導入、出力の `>&2` 保護）
  - [ ] `fxg-server`: コマンドテンプレート型プロビジョナー管理（`~/.flexagent/config.toml` の `[provisioners.*]` からの子プロセス起動、`stderr` の `BootstrapLog` ストリーム配信、アイドルタイムアウト監視）
  - [ ] Git Credential Proxy: 一時VM内の `GIT_ASKPASS` / credential helper 設定と、`GitCredentialRequest` / `GitCredentialResponse` によるオンメモリ認証トークン中継
  - [ ] Graceful Drain & Git Bundle 退避/復元: 破棄前の `DrainAndShutdown` 送信 ➔ 未送信イベント全フラッシュ + `git bundle create` (`WorkspaceBundleUpload`) による `server.db` (`git_bundle_path`) への保存、および別ノードでのバンドル復元 (`restore_git_bundle_b64`)
  - [ ] CLI & UI 連携: `fxg run <agent> --provisioner <name>`, `fxg provisioners list/test`, UI 新規セッション画面での一時VMプロビジョナー選択と Chat ペインの「Environment Bootstrap Log」折りたたみカード表示
- **完了条件 / 検証**:
  - プロビジョナー経由で一時コンテナ/VMが起動し、`stderr` のブートストラップログがUIにリアルタイム表示された後、`stdout` JSON Lines でセッションが開始され、終了時に `git bundle` が中央サーバーへ退避・別ノードで復元できること。
- **実装ログ / 進捗メモ**:
  - （実装時に追記）
