# 05. CLI (`fxg`) コマンド仕様・設定ファイルスキーマ・PWA フロントエンド設計

---

## 1. `fxg` CLI コマンド完全リファレンス

> **Status**: 本節は CLI 再編（`session` / `node` / `hub` 体系）後の仕様を先に定義したものです。実装は同じブランチで後続して行うため、現時点のバイナリのコマンドとは一部異なります。

CLI は曖昧な暗黙サブコマンド（`fxg <agent>` 短縮形）を設けず、**`fxg <noun> <verb>` の明示的な体系**に統一します。セッション操作は `session` に集約し、`run` / `ps` のような別名は置きません。プロセスは `node`（ローカル）と `hub`（中央）の2つだけにし、どちらも同じ `start` / `stop` / `restart` / `status` を持ちます。`service` は OS ログイン時の自動起動登録だけを担当し、起動・停止は `node` / `hub` に委譲します。

CLI のパースには **`usage-rs`**（`usage = { package = "usage-rs", version = "7.3", features = ["completions"] }`）を使用し、`#[derive(Cli)]` / `#[derive(Args)]` / `#[derive(Subcommands)]` + `Run` / `RunWith` でコマンドを定義します（`clap` は使用しない）。引数なしの **`fxg` 実行はデフォルトサブコマンドとして `fxg web`（node の Web UI）が実行**され、node が未起動であれば自動でバックグラウンド起動し、トークン付きURLでブラウザを自動オープンします。

`__usage_spec__` が出力する KDL spec を単一のソースとして、シェル補完（Bash / Zsh / Fish / PowerShell / Nushell）・manpage・Markdown リファレンスを `usage` CLI から生成し、本セクションのコマンドリファレンスと同期させます（`usage` CLI は `mise.toml` で固定）。

Web UI をメインのインターフェースとするローカルファースト設計に基づき、`fxg` の CLI サブコマンドは日常的に使うコアコマンド（セッション起動・セッション管理・ノード/ハブのプロセス管理・OS常駐登録・認証・診断）に厳選しています。Web UI 側で完結する管理機能（承認インボックス、論理プロジェクト一覧、Git Worktree一覧、ACPエージェント一覧、VMプロビジョナー一覧）は CLI サブコマンドを廃止し Web UI へ一本化されています（ACP エージェントのログイン/ログアウトだけは対話端末が必要なため `fxg agents login|logout` に残しています）。また、Git連携や一時VM起動等のシステム統合用コマンドは `fxg internal` 配下に集約し非表示 (`#[usage(hide)]`) としています。

### 1.1 設計原則

- **用語**: ローカルプロセスは `node`、中央プロセスは `hub`。コマンド名・設定名に `daemon` / `server` / `central server` は使わない。
- **セッション**: 作成・一覧・操作はすべて `fxg session ...`。`run` / `ps` は廃止。
- **プロセス**: `fxg node start|stop|restart|status` と `fxg hub start|stop|restart|status`。前景起動が既定で、`--detach` でバックグラウンド起動。
- **常駐**: `fxg service install [node|hub]` は OS ログイン時の登録のみ。unit が登録済みなら `node` / `hub` の各コマンドが systemd / launchd / Task Scheduler に委譲する。
- **対象の指定**: フラグではなく位置引数（`fxg web hub`、`fxg logs hub`、`fxg service install hub`）。`--server` のような対象フラグは置かない。
- **緊急停止**: 全ノード一斉の kill switch はハブの Web UI / `POST /api/v1/system/kill-switch` に一本化。CLI の `kill-all` は廃止し、ローカルでは `fxg node stop`（ノードごと停止）を使う。

### 1.2 コマンド一覧

| コマンド                                                                    | 説明                                                                                                                                               | 主なオプション / 引数                                                                                                                                                                                                                            |
| :-------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------- | :----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **`fxg`** (引数なし) / **`fxg web [node\|hub]`**                            | トークン付き URL (`http://127.0.0.1:7860/?token=...`) をデフォルトブラウザで開く（デフォルトサブコマンド）。`hub` を指定するとハブの Web UI を開く | 位置引数: `node` (既定) \| `hub`                                                                                                                                                                                                                 |
| **`fxg session new <agent>`**                                               | カレントディレクトリ（または指定 Worktree / 一時VM）で新規セッションを起動し、Web UI をブラウザで自動オープン                                      | `-p, --prompt <TEXT>`<br/>`--attach <PATH>` (繰り返し可)<br/>`-w, --worktree <BRANCH>`<br/>`--base <BRANCH>`<br/>`--provisioner <NAME>`<br/>`--mode <MODE>`<br/>`--no-open`<br/>`-q, --quiet`: セッション ID だけを出力<br/>`-- <EXTRA_ARGS>...` |
| **`fxg session list`**                                                      | 稼働中・最近のセッション一覧を表示                                                                                                                 | `-a, --all`<br/>`--archived`<br/>`--project <ID>`<br/>`--node <ID>`<br/>`--json`                                                                                                                                                                 |
| **`fxg session show <id>`**                                                 | セッション詳細・状態・Worktree パス・イベント統計を表示                                                                                            | `--events <N>` (既定 20)<br/>`--json`                                                                                                                                                                                                            |
| **`fxg session prompt <id> <text>`**                                        | 既存セッションへ非対話（ヘッドレス）でプロンプトを送信。停止済みはネイティブ復元で自動再開                                                         | `--attach <PATH>`<br/>`--wait`                                                                                                                                                                                                                   |
| **`fxg session cancel <id>`**                                               | 実行中ターンだけをキャンセルし、セッションは維持                                                                                                   | -                                                                                                                                                                                                                                                |
| **`fxg session stop <id>`**                                                 | セッションを正常停止                                                                                                                               | -                                                                                                                                                                                                                                                |
| **`fxg session kill <id>`**                                                 | セッションの子プロセスツリー (Job Object) を強制終了                                                                                               | -                                                                                                                                                                                                                                                |
| **`fxg session resume <id>`**                                               | 停止済みセッションを同じ ID のまま再開                                                                                                             | `--no-open`<br/>`-q, --quiet`<br/>`--replay`                                                                                                                                                                                                     |
| **`fxg session fork <id>`**                                                 | ACP ネイティブの `session/fork` で分岐                                                                                                             | `-w, --worktree <BRANCH>`<br/>`--base <BRANCH>`<br/>`--no-open`<br/>`-q, --quiet`                                                                                                                                                                |
| **`fxg session import <agent-session-id>`**                                 | エージェント側の既存セッションをネイティブ復元で取り込む                                                                                           | `--agent <ID>` (必須)<br/>`--cwd <PATH>`<br/>`--title <TITLE>`                                                                                                                                                                                   |
| **`fxg session archive <id>` / `fxg session unarchive <id>`**               | 一覧からアーカイブ / 復元                                                                                                                          | -                                                                                                                                                                                                                                                |
| **`fxg session delete <id>`**                                               | セッションを削除（会話ログ・承認履歴を消去。**復元不能**）                                                                                         | -                                                                                                                                                                                                                                                |
| **`fxg node start`**                                                        | ローカルノードを前景起動（任意でタスクトレイ常駐）                                                                                                 | `--detach`<br/>`--tray` / `--no-tray`<br/>`--listen <ADDR>` (既定 `127.0.0.1:7860`)<br/>`--hub-url <WS_URL>`<br/>ログ系オプション                                                                                                                |
| **`fxg node stop` / `fxg node restart`**                                    | ローカルノードを停止 / 再起動。service 登録済みなら supervisor に委譲                                                                              | -                                                                                                                                                                                                                                                |
| **`fxg node status`**                                                       | ノードの稼働状態、service 登録、ハブ接続、未同期 Outbox、セッション数を表示                                                                        | `--json`                                                                                                                                                                                                                                         |
| **`fxg node token show` / `fxg node token rotate`**                         | クライアント認証トークン (`auth_token`) の表示 / 再生成                                                                                            | -                                                                                                                                                                                                                                                |
| **`fxg hub start`**                                                         | ハブを前景起動                                                                                                                                     | `--detach`<br/>`--listen <ADDR>` (既定 `0.0.0.0:8080`)<br/>`--port <PORT>`<br/>ログ系オプション                                                                                                                                                  |
| **`fxg hub stop` / `fxg hub restart`**                                      | ハブを停止 / 再起動。service 登録済みなら supervisor に委譲                                                                                        | -                                                                                                                                                                                                                                                |
| **`fxg hub status`**                                                        | ハブの稼働状態、service 登録、待受アドレス、バージョン、接続ノード数を表示                                                                         | `--json`                                                                                                                                                                                                                                         |
| **`fxg hub node-token issue <node-id>` / `revoke <node-id>` / `list`**      | ノード個別トークンの発行・失効・一覧（ハブ上で実行）                                                                                               | -                                                                                                                                                                                                                                                |
| **`fxg service install [node\|hub]` / `fxg service uninstall [node\|hub]`** | OS ログイン時の自動起動を登録 / 解除（既定 `node`）                                                                                                | 位置引数: `node` (既定) \| `hub`                                                                                                                                                                                                                 |
| **`fxg logs [node\|hub]`**                                                  | ノードログ / ハブのログを表示。`--session` 指定時はそのセッションのエージェント stderr                                                             | 位置引数: `node` (既定) \| `hub`<br/>`--session <ID>` (node のみ)<br/>`-f, --follow`<br/>`--lines <N>`                                                                                                                                           |
| **`fxg doctor`**                                                            | 設定・認証トークン・DB・ログ・git のローカル診断                                                                                                   | `--json`                                                                                                                                                                                                                                         |
| **`fxg support bundle`**                                                    | 秘密情報を redaction したサポートバンドルを出力                                                                                                    | `-o, --output <PATH>`                                                                                                                                                                                                                            |
| **`fxg support incidents`**                                                 | 永続化された障害インシデントを新しい順に一覧表示                                                                                                   | `--limit <N>`<br/>`--all`<br/>`--json`                                                                                                                                                                                                           |
| **`fxg agents login <id>` / `fxg agents logout <id>`**                      | ACP エージェントの認証 / ログアウト（ローカルノード）                                                                                              | `--method <ID>`                                                                                                                                                                                                                                  |
| **`fxg internal <subcommand>`**                                             | 内部・システム連携用隠しコマンド（`--help` には非表示）                                                                                            | `stdio`: 標準入出力パイプ (JSON Lines) の一時ノード (`--ephemeral` / `--workspace <DIR>`)<br/>`bootstrap-workspace`: 一時VM内の Zero-Touch 初期化<br/>`git-askpass <PROMPT>`: `GIT_ASKPASS` ヘルパー                                             |

ハブの Web UI を開く場合、ハブの `auth_token`（`~/.flexagent/auth_token`。Docker では
`docker compose exec hub cat /data/auth_token`）を初回入力ダイアログに入力します。

`fxg session diagnose` は廃止しました。状態は `fxg session show`、エージェント stderr は `fxg logs --session <id>` で確認します。

`fxg session prompt` は停止済みセッションをネイティブ復元で自動再開します。非対応エージェントは `RESUME_REQUIRED` を返すため、`fxg session resume <id> --replay` で履歴 Replay 再開してから送信してください。

### 1.3 代表的な CLI 操作例

```bash
# 1. セッションの起動（カレントディレクトリで起動＆Web UI を自動オープン）
fxg session new claude -p "テストを修正して"
fxg session new claude -p "この画面を直して" --attach shot.png
fxg session new antigravity
fxg session new claude -w feat/auth
fxg session new claude --provisioner colab-pro
fxg session new claude -- --model claude-sonnet-4
fxg session new claude --no-open

# 2. セッション一覧・履歴操作
fxg session list
fxg session fork 0195f0...
fxg session resume 0195f0...
fxg session cancel 0195f0...
fxg session stop 0195f0...
fxg session archive 0195f0...
fxg session delete 0195f0...

# 3. エージェント認証・トークン
fxg agents login claude-code
fxg agents logout claude-code
fxg node token show
fxg hub node-token issue home-win

# 4. ノード・ハブ・常駐サービス
fxg node start --tray          # ノードをタスクトレイ常駐で前景起動
fxg node status                # 稼働状態・ハブ接続・Outbox を確認
fxg node stop                  # ノードを停止（全セッションも停止）
fxg hub start --port 8080      # ハブを前景起動
fxg hub status                 # ハブの稼働状態を確認
fxg hub stop                   # ハブを停止
fxg service install            # OS ログイン時のノード自動起動を登録
fxg service install hub        # OS ログイン時のハブ自動起動を登録
fxg web                        # ノードの Web UI を開く
fxg web hub                    # ハブの Web UI を開く

# 5. 診断
fxg logs                       # ノードログ
fxg logs hub                   # ハブのログ
fxg logs --session 0195f0...   # セッションのエージェント stderr
fxg doctor
fxg support bundle -o /tmp/fxg-support.json
fxg support incidents
```

`fxg session ...` / `fxg web` などの IPC を使うコマンドは、ノードが起動していない場合に `fxg node start --detach` 相当で自動起動します（ログ: `~/.flexagent/logs/node.log`）。自動起動を無効化するには `FXG_NO_AUTO_NODE=1` を設定してください。`fxg node start` を前景で起動している場合や、`fxg service install` で常駐させている場合はそちらが優先されます。停止は `fxg node stop` です。

---

## 2. 設定の所有者とディレクトリ構造

設定は「その設定を直接使うプロセス」が唯一の所有者です。実行時に変更する設定は
所有者の DB に型付き構造で保存し、`config.toml` にはプロセス起動時に必要な値だけを
残します。`.fxg.toml`、`custom_agents.json`、`enabled_package_agents.json` は廃止
しました（本番未デプロイのため移行コマンドはありません）。

| 設定                                                            | 所有者 / 保存場所                                                                           | 変更手段                                       |
| :-------------------------------------------------------------- | :------------------------------------------------------------------------------------------ | :--------------------------------------------- |
| 待受アドレス・接続先・ログ・`allowed_hosts` / `allowed_origins` | `config.toml` / `FXG_*` / CLI                                                               | ファイル編集 + 再起動（Web UI は読み取り専用） |
| `node.name` / `allow_remote_pty` / `project_scan_dirs` / `tray` | `node.db` (`node_settings`)                                                                 | Client API（ハブは対象ノードへプロキシ）       |
| エージェント既定値・エイリアス・カスタム定義・npx/uvx 有効化    | `node.db` (`agent_settings` / `agent_aliases` / `custom_agents` / `enabled_package_agents`) | Client API                                     |
| 明示的な `project_key`                                          | `node.db` (`project_path_links`)                                                            | Client API / Web UI                            |
| VAPID subject・Git 認証・Provisioner・一時VM Bootstrap          | `hub.db`                                                                                    | ハブ Web UI                                    |
| 秘密の値                                                        | `~/.flexagent/secrets/{node,hub}/<id>` (`0600`)                                             | Secret API（書き込み専用）                     |
| 言語・テーマ・ピン留め                                          | ブラウザ `localStorage`                                                                     | Web UI                                         |

### 2.1 データディレクトリ構成 (`~/.flexagent/`)

```text
~/.flexagent/
├── config.toml                 # 起動設定のみ (Node / Hub / Log)
├── auth_token                  # Client 認証トークン (0600)
├── node_token                  # Node ペアリングトークン (0600)
├── vapid.json                  # Web Push VAPID 秘密鍵 (Hub用, 0600)
├── node.db                     # ノード所有の設定 + セッション正データ
├── hub.db                      # ハブ所有の設定 + プロジェクト投影
├── cache/registry.json         # ACP Registry キャッシュ
├── secrets/
│   ├── node/<secret-id>        # ノード所有の秘密値 (0600)
│   └── hub/<secret-id>         # ハブ所有の秘密値 (0600)
└── worktrees/<project>/<branch>/  # 固定の Worktree 作成先
```

### 2.2 `config.toml` スキーマ（起動設定のみ）

```toml
[node]
node_id = "home-win"
listen_addr = "127.0.0.1:7860"
hub_url = "ws://100.64.0.10:8080/api/v1/node/ws"

[hub]
listen_addr = "0.0.0.0:8080"
allowed_hosts = ["home-server.tailnet-xxxx.ts.net:8080"]
allowed_origins = ["https://home-server.tailnet-xxxx.ts.net"]

[log]
level = "info"
file = "logs/node.log"
format = "text"
no_color = false
```

`node.name` / `allow_remote_pty` / `project_scan_dirs` / `tray`、`[agents]`、
`[provisioners]`、`[hub.git_credentials]`、`vapid_subject` は `config.toml`
から削除されました。Web UI の設定カタログは現在値と出所
(`config.toml` / `env` / `cli` / `default`) を読み取り専用で表示します。

### 2.3 Worktree と Bootstrap

- Worktree の作成先は `~/.flexagent/worktrees/{project}/{branch}` に固定です。
  `dir_template` / `copy_files` / `post_create` / `--path` は廃止しました。
  Worktree の作成は Web UI、または `fxg session new <agent> -w <branch> [--base <branch>]`
  / `fxg session fork <id> -w <branch>` から行います。
- 一時VMの初期化は `hub.db` の `project_profiles`
  (`bootstrap_setup` / `bootstrap_env`) が所有します。ハブは
  `FXG_PROJECT_PROFILE_B64` を注入し、`fxg internal bootstrap-workspace` は
  `--profile <path>` またはこの環境変数から `ProjectProfile` を読み取ります。
  ツール検出はリポジトリ内の `mise.toml` / `.tool-versions` / マニフェストのみを
  使います。

## 3. Web UI / Android PWA (`ui/`) 設計

PCブラウザ、ローカルフォールバック (`localhost:7860`)、および Android
スマートフォン（ホーム画面追加PWA）のすべてを単一のレスポンシブSPAで提供します。

### 3.1 フロントエンド技術スタック & 開発基盤

- **フレームワーク & ルーティング**: SvelteKit (`Svelte 5` Runes +
  `@sveltejs/adapter-static` による SPA モード) + Vite + TypeScript
- **パッケージマネージャ & タスクランナー**: `pnpm` + `mise` (`mise.toml` による
  `specta` 型生成 ➔ UIビルド ➔ Cargoビルドの一貫タスク管理)
- **リンター・フォーマッター・テスト**:
  - **Lint & Format**: `oxlint` / `oxfmt` (Oxc
    ツールチェインによる高速静的解析・コード整形)
  - **型・テンプレート検査**: `svelte-check`
  - **単体テスト**: `Vitest` (+ `happy-dom`)
- **スタイリング & UIコンポーネント**:
  - **CSS**: Tailwind CSS v4 (`@tailwindcss/vite`)
  - **UIライブラリ**: `shadcn-svelte` (`bits-ui`
    ベースのヘッドレスプリミティブ + モバイルボトムシート用 `vaul-svelte`)
  - **アイコン**: `@lucide/svelte`
    (モバイルの片手操作・ボトムシートUIと、デスクトップのマルチペインUIをレスポンシブ切替)
- **状態管理 & 同期**: Svelte 5 Runes (`$state`, `$derived` を用いた
  `*.svelte.ts` クラスベースのカスタム WebSocket 差分同期ストア /
  **接続先ストア**（ハブ / ローカルノード）の `cursor` を保存し、
  再接続時に `Subscribe { since_cursor }` で差分再開する。**DB
  再作成・リセット時はカーソルの巻き戻りを検知してタイムラインを破棄し、
  0 から全量を再同期する**。 イベントは `event_id`
  / `(session_id, node_seq)` で upsert し、`LiveStreamDelta` と 永続イベントを
  `message_id` でマージする)
- **コード・Diff・双方向ターミナル**:
  - **Diff / コード表示**:
    - PC: `monaco-editor` (Monaco Diff Editor: Side-by-side / Inline
      切替、ミニマップ、構文ハイライト。Svelte の Attachment / Action
      で直接マウント)
    - モバイル: 軽量シンタックスハイライト `shiki` + Unified Diff ビューア
      (折りたたみ・変更行ハイライト)
  - **双方向ターミナル**:
    - **コアエンジン**: `ghostty-web` (WebAssembly版 `libghostty-vt` + Canvas 2D
      レンダラ)
    - **抽象化レイヤー (`ITerminalAdapter`)**:
      将来的なレンダラ差し替え（DOMベースの `wterm`
      など）やテスト容易性を担保する薄いラッパー設計。
    - ノードの ConPTY / Unix PTY と WebSocket (`/api/v1/pty/ws`)
      で直結し、キー入力・リサイズ・ANSIカラー出力を双方向ストリーミング。
    - モバイルPWA向け: 画面下部に `Ctrl`, `Esc`, `Tab`, `↑`, `↓`, `←`, `→`
      などの仮想キーバーを提供。
- **型安全性**: Rustの `fxg-protocol` から `specta`
  で自動生成された型定義 (`$lib/generated/bindings`) をインポート。

### 3.2 主要画面構成

1. **グローバル承認 Inbox 画面 (バッジ通知付き)**:
   - 全ノード・全セッションで現在 `waiting_permission`
     になっているリクエストを一箇所に集約。
   - コマンド実行（例:
     `cargo test`）やファイル変更のDiffをその場でプレビューし、ワンタップで
     **Approve / Allow Always / Reject** を応答。
2. **プロジェクト & Worktree 一覧画面**:
   - 論理プロジェクト（例: `github.com/nazo6/flexagent`）ごとにグループ化。
   - 各ノード上の **Git Worktree
     一覧**（メインリポジトリ、各ブランチ、未コミット差分件数）を可視化。
   - 「＋新規セッション」ボタンから、以下のいずれかを選択して起動：
     - **常駐ノードの既存Worktree**、または
       **「新規Worktree（ブランチ名指定）を作成して起動」**
     - **「＋一時VMをスポーンして起動（Local Docker / Local Incus VM / Google
       Colab Pro）」**（選択したブランチを自動クローンし、`mise`/`uv`
       で必要ツールを自動セットアップして開始）
   - 不要になったWorktreeの削除もここから実行。ACPネイティブForkに対応した
     エージェントのセッションは、セッション詳細の Fork から同じノード上に分岐できる。
3. **セッション詳細（チャット & ワークスペース）画面**:
   - **ヘッダー**: 実行ノード名（例: `Home-Win` または
     `Colab Pro [Ephemeral]`）、接続状態、バインドされているWorktree（例:
     `feat/auth`）を常時表示。あわせて **同期状態バッジ**（未同期イベント件数 /
     最終同期時刻。ハブ停止中は Outbox 残数を明示）を表示する。
     Composer
     からの送信時は、ネイティブ復元対応エージェントなら停止済みセッションでも
     自動で再開して送信される（**Resume
     ボタンは常時表示せず、ネイティブ復元非対応 (`RESUME_REQUIRED`)
     を検出したときだけ表示**し、履歴 Replay での再開
     (`POST /api/v1/sessions/:id/resume`) を実行できる。設計: docs/04 §4.1.1）。
   - **セッション操作メニュー**: ヘッダーの「…」メニューとサイドバー各行から
     **アーカイブ** (`POST /api/v1/sessions/:id/archive`。一覧から非表示にする
     だけで復元可能) と **削除** (`DELETE /api/v1/sessions/:id`。会話ログ・
     承認履歴を消去し復元不能。確認ダイアログを表示) を実行できる。
     アーカイブ済みセッションはサイドバー下部の「アーカイブ済み」セクションに
     折りたたんで表示され、セッション画面には復元バナーが表示される。
   - **マルチペイン / タブ構成** (デスクトップは左右分割、モバイルはタブ切替):
     - **Chat ペイン**:
       - 一時VM起動時（`provisioning` / `bootstrapping`
         状態）は、最上部に折りたたみ式の **「Environment Bootstrap
         Log」カード** を表示し、VM起動・`git clone`・`mise` ツール自動導入の
         `stderr` 出力をリアルタイム表示。
       - ストリームタイムライン（ユーザー発言、思考プロセス折りたたみ、ツール実行、承認カード、Pending
         Queue）。
     - **Diff ペイン (2段階スコープ切替)**:
       - **「セッションの変更 (This Session)」**:
         このセッションでエージェントが編集したファイル一覧とDiff（ノードがオフラインでもハブから100%閲覧可能）。
       - **「ノードのGit作業ツリー (Worktree Diff)」**:
         ノードにリアルタイム問い合わせる最新差分。
         - `(●) vs Base (main...HEAD)`:
           ベースブランチとの累積差分（PRレビュー感覚で全体の変更を把握）。
         - `( ) vs HEAD (Uncommitted)`: 作業ツリーの未コミット差分。
         - ファイルツリー（変更行数バッジ `+45 -2`）からファイルを選択して
           Monaco Diff Editor で閲覧。
     - **Terminal ペイン (統合 Web PTY)**:
       - 画面下部ドロワー（開閉可能）または独立タブで展開。
       - セッションの作業ディレクトリ（Worktree）上で動作するシェル（PowerShell
         / bash / zsh）を直接対話操作。
       - エージェントが実行している対話型コマンドへのキー入力送信（`TerminalInput`）にもシームレスに切り替え可能。
   - **動的コントロールバー**:
     - スラッシュコマンド補完、モード切替 (`plan` /
       `code`)、モデル選択、中断ボタン。
     - モード / モデル等の設定セレクタは「項目名 +
       現在値」を常時表示し、項目名で
       何を選択するのかを明示する。現在値は選択肢と同じ表示名
       (エージェントが返す
       `name`) に揃え、未選択時は「既定」を表示する。
4. **接続先スイッチャー（耐障害性サポート）**:
   - 通常はハブへ接続しますが、万が一ハブがダウンしている場合は、画面上部のバナーから登録済みの各ノードのローカルWeb
     UI（`http://localhost:7860`）へワンタップで接続先を切り替え。
5. **緊急キルスイッチ & セキュリティ設定**:
   - 画面ヘッダーに常時表示される **「緊急停止 (Kill Switch)」**
     ボタン。タップ時に確認モーダルを表示し、全ノードで稼働中の全セッション・プロセスツリー・PTYを即時強制停止。
   - 初回アクセス時または未認証時に表示される
     **「認証トークン入力ダイアログ」**（入力成功時に `fxg_session` Cookie
     を自動保持）。
   - ノード側でリモートPTYが無効化（`allow_remote_pty = false`）されている場合、Terminalペインに「リモートPTYはセキュリティポリシーにより無効化されています。ローカル端末（`localhost:7860`
     または CLI）からご利用ください」と安全にフォールバック表示。
6. **設定画面（タブ構成）**:
   - **全般**:
     クライアント認証トークンの再生成とノードペアリング（ノード個別トークンの発行・失効）。
   - **エージェント管理**: ノードで利用可能なエージェント (fxg 管理 / PATH
     上の既存インストール / npx・uvx / カスタム) を一覧表示し、カタログからの
     明示インストール・更新・削除と、カスタム ACP
     エージェントの追加・編集・削除を行う。`npx` / `uvx`
     配布は既定では一覧に出さず、「カタログから追加」でエージェントごとに
     有効化・無効化する (状態はノードローカルに保存され、新規セッションの
     選択肢にも反映される)。
   - **監査ログ**: 誰が・いつ・どのLAN/VPN
     IPからどの承認操作やWorktree作成を行ったかをタイムライン形式で確認。

### 3.3 Android PWA & Web Push (VAPID) の実装詳細

- **`manifest.webmanifest`**: `"display": "standalone"`,
  `"theme_color": "#0f172a"`
  を設定し、Androidでネイティブアプリ同等のフルスクリーン起動を実現。
- **Service Worker (`src/service-worker.ts` / `$service-worker`)**:
  1. **App Shell キャッシュ**: SvelteKit 標準の `$service-worker`
     モジュール（`build`, `files`,
     `version`）を用いてHTML/JS/CSSをキャッシュし、モバイル回線が不安定な場所やハブ障害時でもUIが即座に立ち上がるようにします。
     ナビゲーションはネットワーク優先 (オフライン時のみ `index.html`
     へフォールバック) で、
     `/api/**` は常にネットワークへ素通しします。
  2. **Web Push 受信 & アクションボタン**: ハブからVAPID Web
     Pushを受信した際、Android通知バナーに以下を表示します：
     - タイトル: `[flexagent] 承認リクエスト (Home-Win)`
     - 本文: `claude: Run "cargo test --workspace"`
     - アクションボタン: **`[Approve]`** / **`[Reject]`**
       ユーザーが通知バナー上の `[Approve]` をタップすると、Service
       Workerがバックグラウンドで
       `POST /api/v1/sessions/:id/permissions/:req_id/respond`
       （`{ selected_option_id: "allow_once" | "reject", resolved_by: "android_push" }`）
       を叩き、アプリ画面を開くことすらなく承認が完了します。既に解決済み
       (`ALREADY_RESOLVED`) の場合は正常終了として扱います。

     Push ペイロードは `PushNotificationPayload`
     (`title` / `body` / `session_id` / `request_id` / `tag` /
     `allow_option_id` / `reject_option_id`) で、VAPID 鍵はハブが
     `~/.flexagent/vapid.json` に自動生成・永続化します (暗号化は RFC 8291
     aes128gcm / 署名は RFC 8292)。購読は通知許可→`pushManager.subscribe`→
     `POST /api/v1/push/subscribe` の順で確立し、失効した購読 (Push
     サービスの `404` / `410`) は送信時に自動削除されます。

  3. **質問 (elicitation) の通知**: エージェントが質問した場合も
     `[flexagent] 質問 (Home-Win)` として通知します。form の回答はアプリ内
     でしか行えないため、承認のような `[Approve]` / `[Reject]`
     ボタンは付けず、タップでアプリのセッション / Inbox を開きます
     (`allow_option_id` / `reject_option_id` は `null`)。

### 3.4 統合 Web ターミナルと抽象化設計 (`ITerminalAdapter`)

Webターミナルは、GhosttyのネイティブVTエミュレーション精度とCanvas
2D高速描画を持つ **`ghostty-web`**
を標準採用します。同時に、UIコンポーネントと特定のターミナル実装を疎結合にするため、薄い抽象化インターフェースを介して利用します。

```typescript
// ui/src/lib/components/terminal/types.ts
export interface TerminalDimensions {
  cols: number;
  rows: number;
}

export interface ITerminalAdapter {
  mount(element: HTMLElement): void;
  clear(): void;
  write(data: string | Uint8Array): void;
  onData(callback: (data: string) => void): { dispose: () => void };
  onResize(
    callback: (dims: TerminalDimensions) => void,
  ): { dispose: () => void };
  fit(): TerminalDimensions;
  focus(): void;
  dispose(): void;
}
```

- **`GhosttyWebAdapter` (標準実装)**:
  - `@coder/ghostty-web` をラップ。
  - WASMによる高速なANSI/VT100シーケンス解釈とCanvas
    2D描画により、ビルドログ等の大量出力でもUIスレッドをブロックしません。
- **差し替え容易性 (Pluggable)**:
  - 将来的にAndroid
    PWAでのネイティブ文字選択・IME入力の操作性やアクセシビリティを重視したいケースが生じた場合でも、DOMレンダラを採用する
    `wterm` (`@wterm/ghostty`) 等のアダプタへ最小限の修正で切り替え可能です。
- **Svelte コンポーネント (`TerminalView.svelte`)**:
  - Svelte 5 の Attachment (`{@attach ...}`) / `ResizeObserver` による自動
    `fit()` 実行とノード側PTYへの `resize` メッセージ送信。
  - **セッション切替時は `$effect` で `sessionId` の変化を監視し、画面を消去して
    PTY WS を新しいセッションへ接続し直す**（古い接続のイベントは無視する）。
  - モバイル仮想キーバー（タップで `Ctrl`, `Esc`, `Tab`,
    矢印キー等のエスケープコードを送信）の統合。

---

## 4. 段階的実装ロードマップ (Milestones)

詳細なフェーズ別タスクリストおよび実装進捗は
**[`docs/changelog/2026-09-30-implementation-plan.md`](./changelog/2026-09-30-implementation-plan.md)**
で管理します。 本節の Milestone と実装計画の Phase
の対応は各見出しの括弧書きのとおりです （Milestone 1 は計画の Phase 1〜2
にまたがり、Milestone 2〜3 はともに Phase 3 に対応）。

### Milestone 1: コアプロトコル・DB・ローカルノード基盤 & セキュリティ基礎 (実装計画 Phase 1〜2 対応)

- [x] Cargo Workspace の構築 (`fxg-protocol`, `fxg-db`, `fxg-pty`, `fxg-acp`,
      `fxg-node`, `fxg-hub`, `fxg-cli`) および `mise.toml` タスク定義
      (Phase 1 完了: `mise run check` / `types` / `sqlx:prepare` 等)
- [x] `node.db` / `hub.db` のSQLiteマイグレーション実装
      (Phase 1 完了: 単一スキーマ + FTS5 trigram + 投影エンジン。実装は
      `crates/fxg-db/`)
- [ ] Git Remote URL正規化による論理プロジェクト解決 (Web UI のプロジェクト画面)
- [ ] Windows Named Pipe / Unix Domain Socket による `fxg` CLI ⇔ `fxg node`
      ローカルIPC疎通
- [ ] **セキュリティ基礎**:
  - [ ] `fxg node` ローカルAPIの `127.0.0.1:7860`（ループバック）厳格バインド
  - [ ] 認証トークン生成・永続化 (`~/.flexagent/auth_token`)
  - [ ] Axum ミドルウェアによる `Host` ヘッダ検証 (DNS Rebinding 対策)
  - [ ] WebSocket ハンドシェイク時の `Origin` ヘッダ検証 (CSWSH 対策)

### Milestone 2: ACP ドライバ & Windows プロセス管理 (実装計画 Phase 3 対応)

- [ ] `fxg-pty`: ConPTY (`portable-pty`) + Windows Job Object (`win32job`) +
      `which` コマンド解決の実装
- [ ] `fxg-acp`: 公式 ACP Registry (`registry.json` / `agent.json`) のフェッチと
      `binary` / `npx` / `uvx` 起動実装
- [ ] `agent-client-protocol` クレートを用いた `AcpDriver` 実装（`fs/*`,
      `terminal/*`, `request_permission`, `AvailableCommand` 対応）
- [ ] `fxg session new <agent>` でセッションを起動し、Web UI
      が自動オープンされる状態にする

### Milestone 3: 未登録エージェントのローカルコマンド解決 (実装計画 Phase 3 対応)

- [ ] `which` によるシステムの `PATH` 探索と未登録エージェントの自動起動
- [ ] ACP stdio 直結による `AcpDriver` 対話動作

### Milestone 4: ハブ & Store-and-Forward 同期 & セキュリティ (実装計画 Phase 4 対応)

- [x] `fxg hub` の Axum WebSocket Hub 実装
- [x] `fxg node` の Outbox Sync Worker 実装（ハブへのOutbound
      WS接続、切断時のローカル蓄積と再接続時の一括同期）
- [x] ハブ経由でのリモートコマンドルーティング（`StartSession`,
      `SendPrompt`, `RespondPermission`）
- [x] **LAN/VPNセキュリティ & 統制**:
  - [x] Node ⇔ Hub 間のペアリングトークン (`node_token`) 認証
  - [x] `audit_logs` テーブルへの操作監査ログ記録
  - [x] 緊急キルスイッチ (`POST /api/v1/system/kill-switch` /
        `HubToNodeMsg::KillAllSessions`) の配信・プロセスツリー即時終了
  - [x] ノード設定 `allow_remote_pty` によるWeb PTYリモート起動拒否ハンドリング

### Milestone 5: 共通 Web UI / Android PWA & Web Push (実装計画 Phase 5 対応)

- [x] `ui/` (SvelteKit / Svelte 5 SPA + TypeScript + Tailwind v4 +
      `shadcn-svelte` + `pnpm` + `oxlint` / `oxfmt` / `svelte-check` / `Vitest`)
      の構築と `rust-embed` による `fxg` バイナリへの組み込み
      (`fxg-ui-assets` クレート + 共通ルーターの SPA フォールバック)
- [x] セッション画面の実装:
  - [x] チャットタイムライン（思考折りたたみ・スラッシュコマンド補完・承認Inbox）
  - [x] 2階層 Diff ビューア（セッション編集差分 + Worktree リアルタイムGit差分:
        `vs Base` / `vs HEAD`）
  - [x] 双方向 Web ターミナル（`ITerminalAdapter` 抽象化 + `ghostty-web` 実装 +
        ConPTY/Unix PTY WebSocket 直結 + モバイル仮想キーバー）
- [x] プロジェクト & Worktree 管理画面（Worktree 一覧・新規作成・削除）
- [x] **UI セキュリティ機能**:
  - [x] 初回トークン入力・Cookie自動保持 (`POST /api/v1/auth/login`)
  - [x] ヘッダーの緊急停止（キルスイッチ）ボタン
  - [x] 監査ログ一覧画面
  - [x] リモートPTY無効化時の案内バナー表示
- [x] Service Worker (`src/service-worker.ts`) と `web-push-native`
      クレートによる
      VAPID Push通知（Androidバックグラウンド通知＆バナー承認）の実装
- [x] `fxg service install` による各OS自動起動設定の実装

### Milestone 6: 一時VM・サンドボックスノード (`fxg internal stdio` & Zero-Touch Provisioner) (実装計画 Phase 6 対応)

- [x] `fxg internal stdio --ephemeral` による標準入出力パイプ（JSON
      Lines）トランスポート実装
  - `stdin` = `HubToNodeMsg` / `stdout` = `NodeToHubMsg`
    （1行1メッセージ）。ログ (`tracing`) は `stderr` のみに出力し、`stdout`
    をプロトコル専用として保護する。`--workspace <DIR>`
    はクローン済みワークスペースをプロジェクトとして登録し、`StartSession` の
    `local_path` をそのディレクトリに固定する。
- [x] `fxg hub` のコマンドテンプレート型プロビジョナー管理（Docker / Rootless
      Podman / Incus VM / `google-colab-cli` の子プロセス起動と `stderr`
      ブートストラップ進捗配信）
  - `{BOOTSTRAP_SCRIPT}` / `{INSTANCE_NAME}` / `{SESSION_ID}`
    を置換し、`FXG_NODE_ID` / `FXG_GIT_URL` / `FXG_GIT_BRANCH` / 短命
    `FXG_GIT_TOKEN` を注入。`stderr` は `HubWsMessage::BootstrapLog`
    として UI へエフェメラル配信する。
- [x] `fxg internal bootstrap-workspace` の実装（`mise` / `uv`
      単一バイナリの自動取得と、リポジトリ設定ファイルからの完全ステートレスなツール自動構築）
  - `Cargo.toml` / `package.json` / `pyproject.toml` / `mise.toml` /
    `.tool-versions` / `.fxg.toml [bootstrap]`（`.fxg.toml` は廃止。現在は
    `hub.db.project_profiles` の `bootstrap_setup` / `bootstrap_env`）
    からツールを検出して導入し、`$FXG_HOME/bootstrap.env`
    に PATH と環境変数を書き出す（生成スクリプトが `exec fxg internal stdio`
    の直前に `source` する）。出力はすべて `stderr`。
- [x] `GIT_ASKPASS` + `GitCredentialRequest`
      パイププロキシによる一時VM内への秘密鍵・トークン非保持化
  - ブートストラップ中は短命トークンを echo する専用ヘルパー、ノード起動後は
    `fxg internal git-askpass`（ローカルIPC）→ `GitCredentialRequest`
    でオンメモリ中継する。
- [x] `DrainAndShutdown` による一時VM破棄前の未送信イベント完全フラッシュ
  - アイドルタイムアウト（`idle_timeout_secs`）またはセッション終了で Drain
    し、未送信イベントを送信してから `DrainComplete` を返す。Drain
    前クラッシュ時の中間イベント損失は許容仕様。
