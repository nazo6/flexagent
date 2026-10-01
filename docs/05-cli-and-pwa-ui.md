# 05. CLI (`fxg`) コマンド仕様・設定ファイルスキーマ・PWA フロントエンド設計

---

## 1. `fxg` CLI コマンド完全リファレンス

CLI は曖昧な暗黙サブコマンド（`fxg <agent>` 短縮形）を設けず、すべて
**`fxg run <agent>` 等の明示的なサブコマンド体系**に統一します。

CLI のパースには **`usage-rs`**（`usage = { package = "usage-rs", version = "6",
features = ["completions"] }`）を使用し、`#[derive(Cli)]` / `#[derive(Args)]` /
`#[derive(Subcommands)]` + `Run` / `RunWith` でコマンドを定義します（`clap`
は使用しない）。
`__usage_spec__` が出力する KDL spec を単一のソースとして、シェル補完（Bash /
Zsh /
Fish / PowerShell / Nushell）・manpage・Markdown リファレンスを `usage` CLI
から生成し、
本セクションのコマンドリファレンスと同期させます（`usage` CLI は `mise.toml`
で固定）。

### 1.1 コマンド一覧ツリー

| コマンド                             | 説明                                                                                                                                                                              | 主なオプション / 引数                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| :----------------------------------- | :-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **`fxg run <agent>`**                | カレントディレクトリ（または指定Worktree/一時VM）で新規セッションを起動し、即座にターミナルを Attach                                                                              | `-p, --prompt <TEXT>`: 初期プロンプト送信<br/>`-w, --worktree <BRANCH>`: Worktreeを作成/再利用して起動<br/>`--base <BRANCH>`: Worktree新規作成時のベースブランチ<br/>`--provisioner <NAME>`: 一時VM (`local-docker`, `colab-pro` 等) で起動（中央サーバーホスト上で起動。中央サーバー必須）<br/>`--mode <MODE>`: 初期モード (`code`, `plan` 等)<br/>`--acp`: `opencode2` を標準ACPモードで起動<br/>`-d, --detach`: TUIをAttachせずバックグラウンド起動しセッションIDを出力<br/>`-- <EXTRA_ARGS>...`: エージェントプロセスへのパススルー引数 |
| **`fxg attach [session-id]`**        | 稼働中セッションにターミナル（内蔵TUI または OpenCode2 純正TUI）を再接続                                                                                                          | `session-id` 省略時はカレントディレクトリ（Worktree）の直近アクティブセッションに自動接続                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| **`fxg ps`**                         | 稼働中・最近のセッション一覧を表示（`fxg session list` のエイリアス）                                                                                                             | `-a, --all`: 停止済みセッションも含めて表示<br/>`--archived`: アーカイブ済みセッションも含めて表示<br/>`--project <ID>`: 論理プロジェクトIDでフィルタ<br/>`--node <ID>`: ノードIDでフィルタ<br/>`--json`: JSON形式で出力                                                                                                                                                                                                                                                                                                                    |
| **`fxg session list`**               | セッション一覧を表示（`fxg ps` と同一）                                                                                                                                           | `-a, --all`, `--archived`, `--project <ID>`, `--node <ID>`, `--json`                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| **`fxg session show <id>`**          | セッション詳細・状態・Worktreeパス・イベント統計を表示                                                                                                                            | `--json`: JSON形式で出力                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **`fxg session prompt <id> <text>`** | 既存セッションへ非対話（ヘッドレス）でプロンプトを送信（停止済みセッションはネイティブ復元で自動再開。非対応エージェントは `RESUME_REQUIRED` となり `fxg session resume` が必要） | `--wait`: ターン完了まで待機して出力を表示                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **`fxg session stop <id>`**          | 実行中ターンのキャンセル、またはセッションの正常停止                                                                                                                              | `--turn-only`: セッションは維持し現在のターンのみ中断                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| **`fxg session kill <id>`**          | セッションの子プロセスツリー (Job Object) を強制終了                                                                                                                              | -                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg session revert <id>`**        | Shadow Git Tree を用いて指定ターン時点へファイルと会話を巻き戻し                                                                                                                  | `--to-seq <NODE_SEQ>` (必須): 巻き戻し先のイベント連番                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| **`fxg session fork <id>`**          | 指定時点から会話を分岐して新規セッションを作成（別エージェントや一時VMへの引き継ぎ・退避バンドル復元対応）                                                                        | `--from-seq <NODE_SEQ>`: 分岐元の連番（省略時は最新）<br/>`--agent <AGENT>`: 分岐後のエージェント変更<br/>`-w, --worktree <BRANCH>`: 新規Worktreeへ分岐<br/>`--provisioner <NAME>`: 一時VM上で分岐開始                                                                                                                                                                                                                                                                                                                                      |
| **`fxg session resume <id>`**        | 停止済みセッションを同じセッションIDのまま再開（ネイティブ復元優先。非対応エージェントは履歴 Replay で継続）                                                                      | `-d, --detach`: TUIをAttachせず再開しセッションIDを出力<br/>`--replay`: ネイティブ復元を試みず履歴 Replay で継続                                                                                                                                                                                                                                                                                                                                                                                                                            |
| **`fxg session archive <id>`**       | セッションを一覧からアーカイブする（一覧から非表示。イベントログは保持され `unarchive` で復元可能）                                                                               | -                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg session unarchive <id>`**     | アーカイブ済みセッションを一覧へ戻す                                                                                                                                              | -                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg session delete <id>`**        | セッションを削除する（稼働中は停止してから会話ログ・承認履歴を消去。**復元不能**。Worktree のファイルは削除しない）                                                               | -                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg inbox list`**                 | 現在承認待ち (`waiting_permission`) のリクエスト一覧を表示                                                                                                                        | `--json`: JSON形式で出力                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **`fxg inbox approve <req-id>`**     | 承認リクエストを許可 (`allow_once` / `--always` で `allow_always`)                                                                                                                | `--always`: 同一ツール操作をセッション中常時許可                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| **`fxg inbox reject <req-id>`**      | 承認リクエストを却下                                                                                                                                                              | -                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg worktree list`**              | カレントプロジェクトの Git Worktree 一覧と紐づくセッション状態を表示（別名: `fxg wt list`）                                                                                       | `--project <ID>`, `--json`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **`fxg worktree add <branch>`**      | 設定テンプレート先（デフォルト: `~/.flexagent/worktrees/<project>/<branch>`）に新規 Worktree を作成                                                                               | `--base <BASE_BRANCH>`: 起点ブランチ<br/>`--path <DIR>`: 配置先パスの明示指定                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| **`fxg worktree remove <target>`**   | 指定ブランチまたはパスの Worktree を削除 (`git worktree remove`)                                                                                                                  | `-f, --force`: 未コミット変更がある場合も強制削除                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg worktree prune`**             | 削除済みディレクトリの Worktree 管理情報をクリーンアップ                                                                                                                          | -                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg project info`**               | カレントディレクトリの論理プロジェクトID (`project_key`)・Gitルート・Worktree判定結果を表示                                                                                       | `--json`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **`fxg project list`**               | 登録済み論理プロジェクト一覧と各ノードのローカルパス紐付けを表示                                                                                                                  | `--json`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **`fxg project link <project-id>`**  | カレントディレクトリを指定の論理プロジェクトIDに手動紐付け（`.fxg.toml` に保存）                                                                                                  | -                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg project scan [dir]`**         | 指定ディレクトリ配下のGitリポジトリを一括スキャンしデーモンへ登録                                                                                                                 | `dir` 省略時は `config.toml` の `project_scan_dirs` を走査                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **`fxg agents list`**                | 利用可能なエージェント一覧とインストール状態を表示                                                                                                                                | `--all`: ACP Registry 上の全未導入エージェントも表示<br/>`--json`                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg agents install <id>`**        | ACP Registry から指定エージェントを事前ダウンロード・展開                                                                                                                         | `--version <VER>`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg agents update [id]`**         | ACP Registry インデックス (`registry.json`) および導入済みエージェントを更新                                                                                                      | `id` 省略時は全導入済みエージェントを更新                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| **`fxg agents remove <id>`**         | キャッシュ済みの ACP エージェントバイナリを削除                                                                                                                                   | -                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg provisioners list`**          | `config.toml` に定義された一時VMプロビジョナー一覧を表示                                                                                                                          | `--json`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **`fxg provisioners test <name>`**   | 指定プロビジョナーの起動・`fxg daemon --stdio` ハンドシェイク疎通を検証（中央サーバー API 経由で実行。中央サーバー必須）                                                          | -                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **`fxg daemon`**                     | ノードデーモンをフォアグラウンド起動（任意でタスクトレイ常駐）                                                                                                                    | `--listen <ADDR>`: ローカルHTTP/WSバインド先 (既定 `127.0.0.1:7860`)<br/>`--server-url <WS_URL>`: 中央サーバーWS URL<br/>`--allow-remote-pty`: リモートからのWeb PTY起動を許可<br/>`--stdio`: 標準入出力パイプ (JSON Lines) モードで起動<br/>`--ephemeral`: 一時VMモード (自動Drain & Bundle退避有効)<br/>`--workspace <DIR>`: `--stdio` 時の初期対象ディレクトリ<br/>`--tray`: タスクトレイに常駐する (状態表示・自動起動の登録/解除・Web UI を開く・終了)<br/>`--no-tray`: `[node] tray = true` での常駐を一時的に無効化                  |
| **`fxg server`**                     | 中央サーバーをフォアグラウンド起動                                                                                                                                                | `--listen <ADDR>`: バインド先 (既定 `0.0.0.0:8080`)<br/>`--port <PORT>`: ポート番号上書き                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| **`fxg bootstrap-workspace`**        | 一時VM内の Zero-Touch 初期化 (`git clone` + `mise`/`uv` ツール自動導入、出力はすべて `stderr`)                                                                                    | `--repo <GIT_URL>` (必須)<br/>`--branch <BRANCH>`<br/>`--dir <PATH>` (既定 `/tmp/workspace`)                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| **`fxg service <action>`**           | OSログイン時のバックグラウンド常駐サービス管理 (Win/Mac/Linux/WSL)                                                                                                                | `<action>`: `install` \| `uninstall` \| `start` \| `stop` \| `restart` \| `status`<br/>`--server`: `daemon` ではなく `server` を対象にする                                                                                                                                                                                                                                                                                                                                                                                                  |
| **`fxg git-askpass <PROMPT>`**       | `GIT_ASKPASS` ヘルパー (内部用。Git が `Username for '...'` / `Password for '...'` を引数に呼び出し、ローカルIPC 経由で Git Credential Proxy から資格情報を取得して 1 行出力する) | `<PROMPT>`: Git が渡すプロンプト文字列                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| **`fxg auth <action>`**              | 認証トークンの表示・更新                                                                                                                                                          | `<action>`:<br/>`token`: クライアント認証トークン (`auth_token`) を表示<br/>`rotate-token`: `auth_token` を再生成<br/>`node-token issue <node-id>`: ノード個別トークンを発行・表示（中央サーバー上で実行。`nodes.token_hash` にハッシュを保存）<br/>`node-token revoke <node-id>`: ノード個別トークンを失効<br/>`node-token list`: 発行済みノードトークン一覧                                                                                                                                                                               |
| **`fxg web`**                        | トークン付きURL (`http://127.0.0.1:7860/?token=...`) をデフォルトブラウザで開く                                                                                                   | `--server`: ローカルノードではなく中央サーバーURLを開く（中央サーバー用トークンをローカルが持たない場合は、サーバー上で `fxg auth token` を確認して初回入力ダイアログに入力）                                                                                                                                                                                                                                                                                                                                                               |
| **`fxg kill-all`**                   | **【緊急停止】** 全ノードの稼働中セッション・子プロセスツリー・一時VM・PTYを即時強制終了                                                                                          | `--local-only`: 中央サーバーへ配信せずローカルノードのみ停止                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |

### 1.2 代表的なCLI操作例

```bash
# 1. エージェントの起動（カレントディレクトリで起動＆即座にターミナルAttach）
fxg run opencode                                 # opencode2 を起動 (純正TUI + リモート同期)
fxg run antigravity                              # ACP Registry の antigravity-acp を自動取得・起動 (内蔵TUI)
fxg run claude -p "テストを修正して"             # claude-code-acp を初期プロンプト付きで起動
fxg run opencode -w feat/auth                    # ~/.flexagent/worktrees/<project>/feat-auth にWorktreeを作成して起動
fxg run opencode --provisioner colab-pro         # Google Colab Pro を一時VMとしてスポーンしセッション起動
fxg run opencode --provisioner local-incus       # ローカル隔離VM (Incus) をスポーンしセッション起動
fxg run opencode -- --model claude-sonnet-4      # エージェント固有のCLI引数をパススルー

# 2. セッション一覧・再アタッチ・履歴操作
fxg ps                                           # 現在稼働中・最近のセッション一覧を表示
fxg attach                                       # カレントディレクトリの直近セッションに再接続
fxg session revert 0195f0... --to-seq 12         # node_seq=12 の時点へファイルと会話を巻き戻し
fxg session fork 0195f0... --agent antigravity   # 既存セッションを別エージェントへ引き継いで分岐
fxg session resume 0195f0...                     # 停止したセッションを会話コンテキストを維持して再開
fxg session archive 0195f0...                    # セッションを一覧からアーカイブ (復元可能)
fxg session delete 0195f0...                     # セッションを削除 (会話ログを消去。復元不能)

# 3. Worktree・プロジェクト・エージェント管理
fxg worktree list                                # Worktree一覧と稼働セッションを表示
fxg worktree add feat/new-ui --base main         # 新規Worktreeを作成
fxg agents list --all                            # ACP Registry の利用可能エージェント一覧
fxg project info                                 # カレントフォルダの論理プロジェクトIDを確認

# 4. デーモン・サーバー・常駐サービス・緊急停止
fxg daemon --tray                                # デーモンをタスクトレイ常駐で起動
fxg service install                              # OSログイン時のデーモン自動起動を設定
fxg service status                               # デーモン状態・中央サーバー接続・未同期Outbox件数を確認
fxg server --port 8080                           # 中央サーバーを起動
fxg web                                          # トークン付きURLでブラウザを開く
fxg kill-all                                     # 【緊急停止】全セッション・子プロセスツリー・PTYを強制終了
```

---

## 2. 設定ファイルスキーマ & ディレクトリ構造 (`config.toml` / `.fxg.toml`)

設定は **① ユーザーホームのグローバル設定 (`~/.flexagent/config.toml`)** と **②
リポジトリ内のプロジェクト設定 (`.fxg.toml`)** の2階層で構成されます。
ノード設定・サーバー設定・エージェント設定・プロビジョナー定義はすべて単一の
`~/.flexagent/config.toml` に統合され、環境変数 (`FXG_*`)
による上書きをサポートします。

### 2.1 データディレクトリ構成 (`~/.flexagent/`)

ベースディレクトリはデフォルトで `~/.flexagent`（環境変数 `FXG_HOME`
で変更可能）です。

```text
~/.flexagent/
├── config.toml                 # グローバル設定ファイル (Node / Server / Agents / Provisioners)
├── auth_token                  # Client ⇔ Server/Node 認証トークン (パーミッション 0600)
├── node_token                  # このノード専用の Node ⇔ Server ペアリングトークン (サーバーで発行・パーミッション 0600)
├── vapid_private.pem           # Web Push VAPID 秘密鍵 (自動生成・Server用)
├── vapid_public.txt            # Web Push VAPID 公開鍵 (URL-safe Base64)
├── node.db                     # ローカルノード SQLite DB (WALモード)
├── server.db                   # 中央サーバー SQLite DB (WALモード + FTS5)
├── cache/
│   └── registry.json           # ACP Registry インデックスキャッシュ
├── agents/                     # ACP Registry から取得したエージェント実行バイナリ
│   └── <agent-id>/<version>/
├── worktrees/                  # fxg が自動作成する Git Worktree のデフォルト集約ディレクトリ
│   └── <project-slug>/<branch-slug>/
├── snapshots/                  # Shadow Git Tree 用のセッション別一時インデックス (GIT_INDEX_FILE)
│   └── <session-id>.index
└── bundles/                    # 一時VM破棄時に退避された git bundle ファイル
    └── <session-id>.bundle
```

### 2.2 グローバル設定ファイル (`~/.flexagent/config.toml`) スキーマ

すべてのフィールドは省略可能（デフォルト値あり）です。

```toml
# ==============================================================================
# ~/.flexagent/config.toml
# ==============================================================================

# ------------------------------------------------------------------------------
# 1. ローカルノードデーモン設定 (`fxg daemon`)
# ------------------------------------------------------------------------------
[node]
# ノードの一意識別子（省略時は初回起動時にホスト名ベースのスラッグ+短IDを自動採番）
# 環境変数: FXG_NODE_ID
node_id = "home-win"

# UI上に表示されるノード名（省略時はOSのホスト名）
# 環境変数: FXG_NODE_NAME
name = "Home Windows PC"

# ローカルHTTP/WSサーバーのバインドアドレス（セキュリティ原則によりループバック固定推奨）
# 環境変数: FXG_NODE_LISTEN_ADDR
listen_addr = "127.0.0.1:7860"

# 中央サーバーのNode Hub WebSocket URL（未指定時はスタンドアロン・ローカルのみで動作）
# 環境変数: FXG_CENTRAL_SERVER_URL
central_server_url = "ws://100.64.0.10:8080/api/v1/node/ws"

# 中央サーバー接続時のペアリングトークン（ノード個別。中央サーバー上で `fxg auth node-token issue <node-id>` により発行）
# 省略時は ~/.flexagent/node_token を読み込む
# 環境変数: FXG_NODE_TOKEN
# node_token = "..."

# リモート（中央サーバー経由）からの対話型 Web PTY 起動を許可するか（デフォルト: false）
# 環境変数: FXG_ALLOW_REMOTE_PTY
allow_remote_pty = false

# Git Worktree 自動作成時のデフォルト配置先テンプレート
# 利用可能プレースホルダ:
#   {fxg_home}    -> ~/.flexagent
#   {project}     -> スラッシュ等をハイフン正規化した project_key (例: "github.com-nazo6-flexagent")
#   {repo}        -> リポジトリフォルダ名 (例: "flexagent")
#   {repo_parent} -> メインリポジトリの親ディレクトリ絶対パス
#   {branch}      -> スラッシュをハイフンに置換したブランチ名 (例: "feat-auth")
worktree_dir_template = "{fxg_home}/worktrees/{project}/{branch}"

# デーモン起動時および `fxg project scan` 実行時にGitリポジトリを自動探索するディレクトリ一覧
project_scan_dirs = [
  "D:/ghq/github.com",
]

# 各ターンのプロンプト送信直前に Shadow Git Tree スナップショットを自動取得するか
# (ストリーミング途中のチャンクは永続化せず、ターン完了時に完成イベントのみを DB へ書き込む)
snapshot_enabled = true

# `fxg daemon` 起動時にタスクトレイへ常駐するか (デフォルト: false = 無効)
# トレイメニュー: 状態表示 (中央サーバー接続・未同期 Outbox・稼働セッション) /
#                  ログイン時の自動起動トグル / Web UI を開く / 終了
# CLI の `--tray` / `--no-tray` が指定された場合はそちらが優先される
# トレイを作成できない環境 (SSH 等) では警告のみでデーモンは稼働を継続する
tray = false


# ------------------------------------------------------------------------------
# 2. 中央サーバー設定 (`fxg server`)
# ------------------------------------------------------------------------------
[server]
# 中央サーバーのHTTP/WSバインドアドレス（LAN / Tailscale インターフェース等）
# ※ パブリックインターネットへ直接露出せず、ファイアウォール / VPN で隔離された LAN 内でのみ待ち受けること
# 環境変数: FXG_SERVER_LISTEN_ADDR
listen_addr = "0.0.0.0:8080"

# DNS Rebinding 防御 (Host ヘッダ検証) で `localhost` / `127.0.0.1` 以外に許可するホスト名一覧
# 環境変数: FXG_SERVER_ALLOWED_HOSTS (カンマ区切り)
allowed_hosts = [
  "home-server.tailnet-xxxx.ts.net:8080",
  "home-server.tailnet-xxxx.ts.net",
  "192.168.1.50:8080",
]

# CSWSH 防御 (WebSocket Origin 検証) で同一オリジン以外に許可するオリジン一覧
# 環境変数: FXG_SERVER_ALLOWED_ORIGINS (カンマ区切り)
allowed_origins = [
  "https://home-server.tailnet-xxxx.ts.net",
  "http://192.168.1.50:8080",
]

# Web Push (VAPID) の連絡先クレーム (mailto: または https:)
vapid_subject = "mailto:admin@example.com"

# 一時VM (`fxg daemon --stdio`) からの GitCredentialRequest に対する認証プロキシ設定
# ※ ブートストラップ（デーモン起動前の git clone）用には、プロビジョナー起動時に
#    この設定から短命トークン (FXG_GIT_TOKEN) が生成され環境変数として注入される
[server.git_credentials."github.com"]
# "gh_cli" (`gh auth token` から取得) または "env" (指定環境変数から取得)
provider = "gh_cli"
username = "x-access-token"
# provider = "env" の場合に使用する環境変数名:
# token_env = "FXG_GITHUB_PAT"


# ------------------------------------------------------------------------------
# 3. エージェント & ACP Registry 設定
# ------------------------------------------------------------------------------
[agents]
# `fxg run` 等でエージェント指定を省略した場合やUIの初期選択エージェント
default_agent = "opencode2"

# OpenCode2 の起動モード: "bridge" (opencode2 serve + 純正TUI Attach) または "acp" (opencode2 acp)
opencode_mode = "bridge"

# ACP 公式レジストリURLとキャッシュ有効期間 (秒)
registry_url = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json"
registry_cache_ttl_secs = 86400

# `fxg run <alias>` で使用できるエージェント名エイリアス（デフォルト内蔵値の上書き・追加）
[agents.aliases]
opencode = "opencode2"
claude = "claude-code-acp"
antigravity = "antigravity-acp"
gemini = "gemini-cli-acp"
codex = "codex-acp"

# レジストリ外のカスタムACPエージェント定義（ローカルコマンドまたは外部 agent.json 指定）
[agents.custom.my-local-agent]
name = "My Custom ACP Agent"
command = "node"
args = ["D:/tools/my-agent/dist/acp.js"]
env = { LOG_LEVEL = "info" }


# ------------------------------------------------------------------------------
# 4. 一時VM・サンドボックスプロビジョナー設定 (`--provisioner <name>`)
# ------------------------------------------------------------------------------
# ※ プロビジョナーは中央サーバー (`fxg server`) ホスト上で子プロセスとして起動される
#    (CLI の `fxg run --provisioner` もサーバー API 経由。サーバー停止中は利用不可)
# 利用可能プレースホルダ:
#   {BOOTSTRAP_SCRIPT} -> `fxg bootstrap-workspace` + `exec fxg daemon --stdio --ephemeral` の自動生成スクリプト
#   {INSTANCE_NAME}    -> 一時インスタンス識別名 (例: "fxg-eph-0195f0...")
#   {SESSION_ID}       -> 起動対象セッションID

[provisioners.local-docker]
description = "Local isolated Ubuntu container (Docker)"
command = "docker"
args = ["run", "--rm", "-i", "ubuntu:24.04", "sh", "-c", "{BOOTSTRAP_SCRIPT}"]
idle_timeout_secs = 900
env = {}

[provisioners.local-incus]
description = "Local KVM MicroVM / LXC (Incus)"
command = "incus"
args = ["launch", "--ephemeral", "images:ubuntu/24.04", "{INSTANCE_NAME}", "--", "sh", "-c", "{BOOTSTRAP_SCRIPT}"]
idle_timeout_secs = 900
env = {}

[provisioners.colab-pro]
description = "Google Colab Pro (T4 GPU Runtime)"
command = "uvx"
args = ["google-colab-cli", "ssh", "--gpu", "t4", "--command", "{BOOTSTRAP_SCRIPT}"]
idle_timeout_secs = 900
env = {}
```

### 2.3 プロジェクト設定ファイル (`.fxg.toml`) スキーマ

各Gitリポジトリのルート（またはモノレポ内のサブディレクトリ）に任意で配置し、プロジェクト固有の挙動を定義します。

```toml
# ==============================================================================
# .fxg.toml (リポジトリルートに配置・Git管理可能)
# ==============================================================================

# 論理プロジェクトIDの明示指定（省略時は git remote.origin.url から自動正規化）
# project_key = "github.com/nazo6/flexagent"

# UI上のプロジェクト表示名（省略時はディレクトリ名またはリポジトリ名）
name = "flexagent"

# ------------------------------------------------------------------------------
# 1. プロジェクト別のデフォルトエージェント設定
# ------------------------------------------------------------------------------
[agent]
default_agent = "opencode2"
default_mode = "code"
extra_args = []

# ------------------------------------------------------------------------------
# 2. Git Worktree 作成時のプロジェクト固有ルール
# ------------------------------------------------------------------------------
[worktree]
# 新規Worktree作成時のデフォルト起点ブランチ（省略時は現在のHEAD）
base_branch = "main"

# このプロジェクト専用のWorktree配置先テンプレート上書き（省略時は config.toml の設定を使用）
# dir_template = "{fxg_home}/worktrees/{project}/{branch}"

# 新規Worktree作成時にメインWorktreeから自動コピーする未追跡ファイル（.env 等）
copy_files = [
  ".env",
  ".env.local",
]

# 新規Worktree作成直後にそのディレクトリで自動実行する初期化コマンド
post_create = [
  "pnpm install --prefer-offline",
]

# ------------------------------------------------------------------------------
# 3. 一時VM (`fxg bootstrap-workspace`) 用の追加セットアップ定義
# ------------------------------------------------------------------------------
[bootstrap]
# リポジトリから自動検出されるツール以外に `mise install` で追加導入するツール
tools = [
  "rust@stable",
  "node@22",
  "pnpm@latest",
]

# `git clone` とツール導入完了後に実行する初期セットアップコマンド（出力はすべて stderr へ配信）
setup = [
  "cargo fetch",
  "pnpm install --frozen-lockfile",
]

# セットアップおよびエージェント実行時に注入する非機密の環境変数
[bootstrap.env]
RUST_BACKTRACE = "1"
```

---

## 3. Web UI / Android PWA (`ui/`) 設計

PCブラウザ、ローカルフォールバック (`localhost:7860`)、および Android
スマートフォン（ホーム画面追加PWA）のすべてを単一のレスポンシブSPAで提供します。

### 3.1 フロントエンド技術スタック & 開発基盤

- **フレームワーク & ルーティング**: SvelteKit (`Svelte 5` Runes +
  `@sveltejs/adapter-static` による SPA モード) + Vite + TypeScript
- **パッケージマネージャ & タスクランナー**: `pnpm` + `mise` (`mise.toml` による
  `ts-rs` 型生成 ➔ UIビルド ➔ Cargoビルドの一貫タスク管理)
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
  **接続先ストア**（中央サーバー / ローカルノード）の `cursor` を保存し、
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
- **型安全性**: Rustの `fxg-protocol` から `ts-rs`
  で自動生成された型定義をインポート。

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
   - 既存セッションの別ノード（または一時VM）へのContext
     Forkや、一時VM破棄時に退避された `git bundle`
     からの復元、不要になったWorktreeの削除もここから実行。
3. **セッション詳細（チャット & ワークスペース）画面**:
   - **ヘッダー**: 実行ノード名（例: `Home-Win` または
     `Colab Pro [Ephemeral]`）、接続状態、バインドされているWorktree（例:
     `feat/auth`）を常時表示。あわせて **同期状態バッジ**（未同期イベント件数 /
     最終同期時刻。中央サーバー停止中は Outbox 残数を明示）を表示する。
     Composer
     からの送信時は、ネイティブ復元対応エージェントなら停止済みセッションでも
     自動で再開して送信される（**Resume
     ボタンは常時表示せず、ネイティブ復元非対応 (`RESUME_REQUIRED`)
     を検出したときだけ表示**し、履歴 Replay での再開
     (`POST /api/v1/sessions/:id/resume`) を実行できる。設計: docs/04 §4.3.1）。
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
         このセッションでエージェントが編集したファイル一覧とDiff（ノードがオフラインでも中央サーバーから100%閲覧可能）。
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
   - 通常は中央サーバーへ接続しますが、万が一中央サーバーがダウンしている場合は、画面上部のバナーから登録済みの各ノードのローカルWeb
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
   - **エージェント管理**: ACP Registry
     のエージェントをノードへインストール・更新・削除。
   - **監査ログ**: 誰が・いつ・どのLAN/VPN
     IPからどの承認操作やWorktree作成を行ったかをタイムライン形式で確認。

### 3.3 Android PWA & Web Push (VAPID) の実装詳細

- **`manifest.webmanifest`**: `"display": "standalone"`,
  `"theme_color": "#0f172a"`
  を設定し、Androidでネイティブアプリ同等のフルスクリーン起動を実現。
- **Service Worker (`src/service-worker.ts` / `$service-worker`)**:
  1. **App Shell キャッシュ**: SvelteKit 標準の `$service-worker`
     モジュール（`build`, `files`,
     `version`）を用いてHTML/JS/CSSをキャッシュし、モバイル回線が不安定な場所や中央サーバー障害時でもUIが即座に立ち上がるようにします。
     ナビゲーションはネットワーク優先 (オフライン時のみ `index.html`
     へフォールバック) で、
     `/api/**` は常にネットワークへ素通しします。
  2. **Web Push 受信 & アクションボタン**: 中央サーバーからVAPID Web
     Pushを受信した際、Android通知バナーに以下を表示します：
     - タイトル: `[flexagent] 承認リクエスト (Home-Win)`
     - 本文: `opencode2: Run "cargo test --workspace"`
     - アクションボタン: **`[Approve]`** / **`[Reject]`**
       ユーザーが通知バナー上の `[Approve]` をタップすると、Service
       Workerがバックグラウンドで
       `POST /api/v1/sessions/:id/permissions/:req_id/respond`
       （`{ selected_option_id: "allow_once" | "reject", resolved_by: "android_push" }`）
       を叩き、アプリ画面を開くことすらなく承認が完了します。既に解決済み
       (`ALREADY_RESOLVED`) の場合は正常終了として扱います。

     Push ペイロードは `PushNotificationPayload`
     (`title` / `body` / `session_id` / `request_id` / `tag` /
     `allow_option_id` / `reject_option_id`) で、VAPID 鍵は中央サーバーが
     `~/.flexagent/vapid.json` に自動生成・永続化します (暗号化は RFC 8291
     aes128gcm / 署名は RFC 8292)。購読は通知許可→`pushManager.subscribe`→
     `POST /api/v1/push/subscribe` の順で確立し、失効した購読 (Push
     サービスの `404` / `410`) は送信時に自動削除されます。

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

### Milestone 1: コアプロトコル・DB・ローカルデーモン基盤 & セキュリティ基礎 (実装計画 Phase 1〜2 対応)

- [x] Cargo Workspace の構築 (`fxg-protocol`, `fxg-db`, `fxg-pty`, `fxg-acp`,
      `fxg-node`, `fxg-server`, `fxg-cli`) および `mise.toml` タスク定義
      (Phase 1 完了: `mise run check` / `types` / `sqlx:prepare` 等)
- [x] `node.db` / `server.db` のSQLiteマイグレーション実装
      (Phase 1 完了: 単一スキーマ + FTS5 trigram + 投影エンジン。実装は
      `crates/fxg-db/`)
- [ ] Git Remote URL正規化による論理プロジェクト解決 (`fxg project info`)
- [ ] Windows Named Pipe / Unix Domain Socket による `fxg` CLI ⇔ `fxg daemon`
      ローカルIPC疎通
- [ ] **セキュリティ基礎**:
  - [ ] `fxg daemon` ローカルAPIの `127.0.0.1:7860`（ループバック）厳格バインド
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
- [ ] `fxg run <acp-agent>` でローカルCLI (TUI)
      からACPエージェントを対話実行・Attachできる状態にする

### Milestone 3: OpenCode2 ハイブリッド統合 (実装計画 Phase 3 対応)

- [ ] `OpenCode2Driver` の実装（`opencode2 serve`
      の起動・SSEイベント購読・OpenAPI操作）
- [ ] `fxg run opencode` 実行時の純正TUI
      Attach（`opencode2 run --attach`）とデーモン側イベント記録の同時動作

### Milestone 4: 中央サーバー & Store-and-Forward 同期 & セキュリティ (実装計画 Phase 4 対応)

- [x] `fxg server` の Axum WebSocket Hub 実装
- [x] `fxg daemon` の Outbox Sync Worker 実装（中央サーバーへのOutbound
      WS接続、切断時のローカル蓄積と再接続時の一括同期）
- [x] 中央サーバー経由でのリモートコマンドルーティング（`StartSession`,
      `SendPrompt`, `RespondPermission`）
- [x] **LAN/VPNセキュリティ & 統制**:
  - [x] Node ⇔ Server 間のペアリングトークン (`node_token`) 認証
  - [x] `audit_logs` テーブルへの操作監査ログ記録
  - [x] 緊急キルスイッチ (`POST /api/v1/system/kill-switch` /
        `ServerToNodeMsg::KillAllSessions`) の配信・プロセスツリー即時終了
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

### Milestone 6: 一時VM・サンドボックスノード (`fxg daemon --stdio` & Zero-Touch Provisioner) (実装計画 Phase 6 対応)

- [x] `fxg daemon --stdio --ephemeral` による標準入出力パイプ（JSON
      Lines）トランスポート実装
  - `stdin` = `ServerToNodeMsg` / `stdout` = `NodeToServerMsg`
    （1行1メッセージ）。ログ (`tracing`) は `stderr` のみに出力し、`stdout`
    をプロトコル専用として保護する。`--workspace <DIR>`
    はクローン済みワークスペースをプロジェクトとして登録し、`StartSession` の
    `local_path` をそのディレクトリに固定する。
- [x] `fxg server` のコマンドテンプレート型プロビジョナー管理（Docker / Rootless
      Podman / Incus VM / `google-colab-cli` の子プロセス起動と `stderr`
      ブートストラップ進捗配信）
  - `{BOOTSTRAP_SCRIPT}` / `{INSTANCE_NAME}` / `{SESSION_ID}`
    を置換し、`FXG_NODE_ID` / `FXG_GIT_URL` / `FXG_GIT_BRANCH` / 短命
    `FXG_GIT_TOKEN` を注入。`stderr` は `ServerWsMessage::BootstrapLog`
    として UI へエフェメラル配信する。
- [x] `fxg bootstrap-workspace` の実装（`mise` / `uv`
      単一バイナリの自動取得と、リポジトリ設定ファイルからの完全ステートレスなツール自動構築）
  - `Cargo.toml` / `package.json` / `pyproject.toml` / `mise.toml` /
    `.tool-versions` / `.fxg.toml [bootstrap]`
    からツールを検出して導入し、`$FXG_HOME/bootstrap.env`
    に PATH と環境変数を書き出す（生成スクリプトが `exec fxg daemon --stdio`
    の直前に `source` する）。出力はすべて `stderr`。
- [x] `GIT_ASKPASS` + `GitCredentialRequest`
      パイププロキシによる一時VM内への秘密鍵・トークン非保持化
  - ブートストラップ中は短命トークンを echo する専用ヘルパー、デーモン起動後は
    `fxg git-askpass`（ローカルIPC）→ `GitCredentialRequest`
    でオンメモリ中継する。
- [x] `DrainAndShutdown` & `WorkspaceBundleUpload`
      による一時VM破棄前の未送信イベント完全フラッシュと `git bundle` 退避・復元
  - アイドルタイムアウト（`idle_timeout_secs`）またはセッション終了で Drain
    し、未コミット変更をスナップショットコミット化した `git bundle` を
    8 MiB 単位に分割転送して `~/.flexagent/bundles/<session-id>.bundle`
    へ保存する。別ノードへの Context Fork は `restore_git_bundle_b64`
    で復元する。Drain 前クラッシュ時の中間イベント損失は許容仕様。
