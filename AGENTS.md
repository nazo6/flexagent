# AGENTS.md

**FlexAgent (`fxg`)**
は、ACP対応エージェントを任意の場所から操作・同期できる、セルフホスト型・ローカルファーストのエージェントマネージャです。

## 1. アーキテクチャ・実装の要点

- **単一バイナリ構成 (`fxg`)**: CLI、ノード (`fxg node start`)、ハブ (`fxg hub start`)、および Web UI / PWA
  (`rust-embed`) を単一の Rust バイナリに内包する。
- **ローカルファースト & Outbox 同期**:
  セッションとイベントの一次ソースは各ノードの `node.db` (SQLite
  WAL)。イベントIDには `UUID v7` とセッション内連番 `node_seq`
  を用い、ハブ (`hub.db`) へは Store-and-Forward (Outbox パターン)
  で非同期同期する。ストリーミングチャンクはメモリで即時配信しつつ、DBへは500msごとまたはターン区切りで結合フラッシュする。
- **同一プロトコル・同一API**:
  - Node ⇔ Hub 間通信は、常駐ノード (Outbound WS) も一時VMノード
    (`fxg internal stdio` の JSON Lines) も同一の `NodeToHubMsg` /
    `HubToNodeMsg` を使用する。
  - Client ⇔ Hub (`:8080`) と Client ⇔ Local Node (`127.0.0.1:7860`) は同一の
    REST API / WebSocket フォーマットを提供する。
- **一時VM (`fxg internal stdio`) の出力分離**: ブートストラップや `mise` / `uv`
  によるツール導入ログはすべて `stderr` (`>&2`) に出力し、`stdout`
  はプロトコル通信 (JSON Lines) 専用として保護する。
- **Windows ファーストクラス対応**:
  - 子プロセス・PTY は必ず **Windows Job Object (`win32job`)**
    にバインドし、親終了時にプロセスツリーごと確実に終了させる。
  - コマンド解決には `which::which_in` (`PATHEXT` 対応) を使用し、パス正規化には
    UNC プレフィックス (`\\?\`) を回避するため必ず **`dunce::canonicalize`**
    を使用する。
- **設定の所有権と秘密ストア**:
  - 実行時に変更する設定は所有者の DB にのみ保存する (`node_settings` /
    `agent_settings` / `custom_agents` / `project_path_links` は `node.db`、
    `hub_settings` / `git_credentials` / `provisioners` / `project_profiles`
    は `hub.db`)。ノード ⇔ ハブ間で設定を同期・マージしない。
  - `config.toml` は待受アドレス・接続先・ログ・セキュリティ境界のみ。
    `.fxg.toml` と旧 JSON 設定ファイルは廃止。
  - 秘密の値は `~/.flexagent/secrets/{node,hub}/<id>` (`0600`) に置き、
    DB にはメタデータだけを保存する。API は書き込み専用で値を返さない。
  - プロジェクト同一性は `project_path_links` → Git リモート → `local:` の順で
    解決し、表示名は自動導出する。Worktree の作成先は
    `~/.flexagent/worktrees/{project}/{branch}` に固定。
- **セキュリティ原則**:
  - `fxg node start` のローカルHTTP/WSは `127.0.0.1:7860` のみにバインドする。
  - 全HTTP/WSエンドポイントでトークン認証 (`Bearer` または
    `HttpOnly; SameSite=Strict` Cookie)、`Host` ヘッダ検証 (DNS
    Rebinding対策)、WS `Origin` ヘッダ検証 (CSWSH対策) を強制する。

## 2. ワークスペース・技術スタック

- **Rust (`crates/`)**:
  - `fxg-protocol`: 共通型・イベント・通信メッセージ定義 (`specta` で
    `ui/src/lib/generated/bindings.ts` へTS型を自動生成)
  - `fxg-db`: SQLite (`sqlx`, WAL, FTS5 `tokenize='trigram'`) による `hub.db`
    / `node.db` 管理
  - `fxg-pty`: ConPTY / Unix PTY (`portable-pty`)、Windows Job Object 管理
  - `fxg-acp`: `AgentDriver` トレイト、`AcpDriver`
    (`agent-client-protocol`)、ACP Registry、ローカル PATH 実行
  - `fxg-node`: ノード (Named Pipe / UDS ローカルIPC、Outbox 同期、Git
    Worktree 管理、Shadow Git Tree (`GIT_INDEX_FILE`)
    によるターン単位のスナップショット/Revert)
  - `fxg-hub`: ハブ (Axum、Node Hub、VAPID Web Push、Stdio
    Provisioner、監査ログ)
  - `fxg-cli`: `fxg` バイナリエントリポイント (セッション開始時に Web UI
    をブラウザで自動オープン)
- **Frontend (`ui/`)**:
  - SvelteKit (Svelte 5 Runes + `@sveltejs/adapter-static` SPA) + TypeScript +
    Tailwind CSS v4 + `shadcn-svelte`
  - ターミナル: `ITerminalAdapter` 抽象化 + `ghostty-web` / Diff:
    `monaco-editor` (PC) & `shiki` (Mobile)
  - ツールチェイン: `pnpm`, `oxlint`, `oxfmt`, `svelte-check`, `Vitest`

## 3. 設計ドキュメント参照先

実装や仕様変更時は以下のドキュメントを参照すること：

- [`docs/README.md`](docs/README.md): コアコンセプト・クレート構成
- [`docs/01-architecture-and-sync.md`](docs/01-architecture-and-sync.md):
  通信トポロジー、Outbox同期、論理プロジェクト・Worktree解決、一時VM
  (`fxg internal stdio`)、セキュリティ
- [`docs/02-database-schema.md`](docs/02-database-schema.md): `hub.db` /
  `node.db` の SQLite スキーマ定義
- [`docs/03-protocol-and-api.md`](docs/03-protocol-and-api.md): `fxg-protocol`
  型定義、WS/Stdio/IPC メッセージ、REST API エンドポイント
- [`docs/04-agent-drivers-and-windows.md`](docs/04-agent-drivers-and-windows.md):
  `AgentDriver`、ACP / `opencode2` 統合、Shadow Git Revert/Fork、Windows
  固有実装
- [`docs/05-cli-and-pwa-ui.md`](docs/05-cli-and-pwa-ui.md): CLI
  コマンド完全リファレンス、設定の所有者 (`config.toml` は起動設定のみ、
  実行時設定は `node.db` / `hub.db`)、秘密ストア、PWA / Web Push 設計
- [`docs/06-type-safety.md`](docs/06-type-safety.md): 型安全性ガイド
  (open enum / `RawJson` / 文字列判定の禁止 / clippy による境界検査)
- [`docs/changelog/2026-09-30-implementation-plan.md`](docs/changelog/2026-09-30-implementation-plan.md):
  フェーズ別の段階的実装プラン・進捗管理

## 4. 開発ルール・ワークフロー

- **DRY 原則の徹底と過剰な後方互換の排除**:
  リファクタリングや仕様変更を行う際、楽だからと冗長な後方互換コード（ラッパー、別名引数、旧仕様のフォールバック等）を追加しないこと。関連する呼び出し元コードも含めて一貫して変更し、常に
  DRY (Don't Repeat Yourself) 原則を重視する。
- **言語ポリシー (コードは英語 / docs は日本語)**:
  - Rust / TypeScript / Svelte を問わず、**コメント・doc コメント・ログ・CLI
    出力・エラーメッセージ・UI 文言はすべて英語**で書く。
  - `fxg-protocol` の doc コメントは `specta` 経由で
    `ui/src/lib/generated/bindings.ts` に出力されるため、英語化の効果は UI
    側の型ドキュメントにも及ぶ (生成物は直接編集しない)。
  - `docs/` と `docs/changelog/` は**日本語のまま維持**する。
  - 多バイト境界のテストなど意図的に日本語を残す行には
    `// allow-japanese: <理由>` を付ける。
  - 英語化は**挙動変更・リファクタと混ぜず、クレート単位の独立したコミット**で行う
    (レビュー可能な差分サイズを保つ)。
  - UI の多言語化は **`paraglide-js`** を前提とする。Rust 側の API エラーは
    `ErrorCode` を i18n キーとして扱い、`message` は英語のフォールバックとして残す
    (サーバーにロケール交渉を持ち込まない)。
- **編集後チェック**: 必要に応じて TS (`svelte-check`, `oxlint`) や Rust
  (`cargo check`, `cargo clippy`) のチェック、lint
  を行う。ファイルを編集した際はかならず format (`mise run fmt`) を行う
  (Rust = rustfmt / TOML = tombi / それ以外 (Markdown・JSON・YAML・HTML・TS・Svelte)
  = oxfmt)。
  また、`fxg-protocol` の型を変更した際は `specta`
  の型出力 (`mise run types`) を同期すること。
- **型安全性 (stringly-typed / 生JSON の禁止)**:
  - ドメイン値 (`kind` / `status` / `role` / `mode` / `client_source` など) を
    `String` で持たず、外部境界で enum に変換する。未知値は
    `Unknown(String)` で保持し、既定値に丸めない。
  - `serde_json::Value` は境界専用。`RawJson` (Phase 2) または
    理由付きの file-level `#![allow(clippy::disallowed_types)]` を置いた
    ファイルでのみ使う。
  - `kind.starts_with(...)` / `status == "..."` のような文字列判定を書かない。
    enum のメソッド (`is_always()` など) を使う。
  - `mise run lint:errors` を通す。既存違反の allow は理由付きで記録し、
    移行フェーズで削除する。
  - 詳細: [`docs/06-type-safety.md`](docs/06-type-safety.md)
- **変更履歴 (Changelog) と実装プランの記録**:
  - **初期実装期間中 (Phase 1 〜 Phase 6)**: 新しい changelog
    ファイルは作成せず、[`docs/changelog/2026-09-30-implementation-plan.md`](docs/changelog/2026-09-30-implementation-plan.md)
    を参照し、実装の進行に合わせて同ファイル内のチェックリストおよび「実装ログ /
    進捗メモ」を順次更新すること。
  - **初期実装完了後**: 大きな機能追加・リファクタリング時は
    `docs/changelog/YYYY-MM-DD-title.md`
    に記録する（同セッション内の更新は同ファイルに追記）。冒頭に **日付・対象パッケージ・対象スクリプト** を明記すること。
