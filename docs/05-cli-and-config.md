# 05. CLI (`fxg`) 設計・設定所有権・ディレクトリ構成

---

## 1. CLI の設計

CLI のコマンド・引数の正は `fxg --help` / `fxg <noun> --help` と
`__usage_spec__` です。このドキュメントでは個別のコマンド一覧を複製せず、
設計原則とコマンド群だけを説明します。

### 1.1 設計原則

- **用語**: ローカルプロセスは `node`、中央プロセスは `hub`。コマンド名・設定名に
  `daemon` / `server` / `central server` は使わない。
- **セッション**: 作成・一覧・操作はすべて `fxg session ...`。
- **プロセス**: `fxg node start|stop|restart|status` と
  `fxg hub start|stop|restart|status`。前景起動が既定で、`--detach` で
  バックグラウンド起動。
- **常駐**: `fxg service install [node|hub]` は OS ログイン時の登録のみ。
  unit が登録済みなら `node` / `hub` の `start` / `stop` / `restart` が
  systemd / launchd / Task Scheduler に委譲する。
- **対象の指定**: フラグではなく位置引数（`fxg logs hub`、
  `fxg service install hub`）。
- **緊急停止**: 全ノード一斉の kill switch はハブの Web UI /
  `POST /api/v1/system/kill-switch` に置く。ローカルのノードは
  `fxg node stop` で停止する。

### 1.2 コマンド群

- `fxg`: サブコマンドを指定せずに実行するとヘルプを表示する（プロセスは起動しない）。
- `fxg node web` / `fxg hub web`: トークン付き URL を表示し、既定ブラウザで
  それぞれの Web UI を開く。`fxg hub web` はその PC のハブだけを開く。
- `fxg session ...`: セッションの作成・一覧・表示・プロンプト・停止・再開・
  Fork・Import・Archive・Delete。
- `fxg node ...`: ローカルノードの起動・停止・状態・ペアリング・Web
  アクセストークン。ノードの接続先は `<state>/node/node.addr` だけで解決し、
  設定ファイルのアドレスは停止時の表示にだけ使う。
- `fxg hub ...`: ハブの起動・停止・状態・ノードペアリングトークン。
  これらは**その PC のハブプロセスだけ**を対象にし、ノードがペアリングして
  いるハブ（リモートを含む）は操作しない。接続先ハブの解決は
  `fxg session new --provisioner` などハブへ直接接続する操作に限る。
- `fxg service ...`: OS ログイン時の自動起動登録。
- `fxg logs` / `fxg doctor`: ログ・自己診断。
- `fxg agents login|logout`: ACP エージェントの認証。
- `fxg internal ...`: システム連携用の隠しコマンド。

`fxg session ...` などのローカル API を使うコマンドは、ノードが起動していない
場合に自動起動せず、`fxg node start --detach` を案内するエラーで終了します。
暗黙にバックグラウンドプロセスを増やさないため、`fxg node start` または
`fxg service install node` で明示的に起動してください。

### 1.3 出力の規約

- **チャネル**: コマンドの結果（一覧・詳細・JSON）は stdout、起動バナー・
  進捗・診断は stderr。パイプやリダイレクトで人間向けの装飾が混ざらないことを
  優先する。
- **意味と描画の分離**: コマンドは色名ではなく役割（`Heading` / `Key` /
  `Muted` / `Accent` / `AccentAlt` / `Success` / `Warning` / `Error`）を指定し、
  実際のスタイルは `crates/fxg-cli/src/output/theme.rs` が決める。配色を変える
  ときはこのファイルだけを触る。`Theme::plain` は全役割を空スタイルに解決する
  ため、色なし出力はエスケープを後から剥がすのではなく最初から生成しない。
- **配色**: 端末のパレットに追従する 16 色の名前付き色を使う。truecolor を
  固定すると light / dark のどちらかで読みにくくなるため、UI の Desk トークン
  とは**語彙**（signal / ok / warn / danger / muted）を揃え、値は共有しない。
- **色の有効化**: `--no-color`、`node.toml` / `hub.toml` の `log.no_color`、
  `NO_COLOR` を起動時に 1 回だけ解決する。それ以外の TTY 判定、パイプ時の
  ANSI 除去、色深度の降格は `anstream` に任せ、個々の出力関数は bool を
  受け取らない。
- **表**: `comfy-table` に Unicode 幅と端末幅への折り返しを任せ、呼び出し側は
  `Cell` と `Role` だけを渡す。
- **JSON**: `--json` は `OutputFormat` で表し、機械可読出力には装飾を付けない。
- **ヘルプ**: `--help` は usage-rs の生成物（プレーンテキスト）で、このテーマ
  層の対象外。

## 2. 設定の所有者とディレクトリ構造

設定は「その設定を直接使うプロセス」が唯一の所有者です。実行時に変更する設定は
所有者の DB に型付き構造で保存し、`node.toml` / `hub.toml` にはプロセス起動時に
必要な値だけを残します。

| 設定                                                         | 所有者 / 保存場所                                                | 変更手段                                          |
| :----------------------------------------------------------- | :--------------------------------------------------------------- | :------------------------------------------------ |
| 待受アドレス・ログ・`allowed_hosts` / `allowed_origins`      | `node.toml` / `hub.toml` / `FXG_*` / CLI                         | ファイル編集 + 再起動（Web UI は読み取り専用）    |
| ハブ接続先・ペアリングトークン                               | `node.db` (`node_pairing`) + `<data>/node/node_token` (`0600`)   | Client API / Web UI（ハブは対象ノードへプロキシ） |
| `node.name` / `project_scan_dirs` / `tray` / `vapid_subject` | `node.db` (`node_settings`)                                      | Client API（ハブは対象ノードへプロキシ）          |
| エージェント既定値・エイリアス・カスタム定義                 | `node.db` (`agent_settings` / `agent_aliases` / `custom_agents`) | Client API                                        |
| 明示的な `project_key`                                       | `node.db` (`project_path_links`)                                 | Client API / Web UI                               |
| VAPID subject（ハブ用）・Provisioner                         | `hub.db`                                                         | ハブ Web UI                                       |
| 秘密の値                                                     | `<data>/{node,hub}/secrets/<id>` (`0600`)                        | Secret API（書き込み専用）                        |
| 言語・テーマ・ピン留め                                       | ブラウザ `localStorage`                                          | Web UI                                            |

### 2.1 データディレクトリ構成

パスは OS ごとの標準ルートに分かれ、その下を `node` / `hub` のロールで
分けます。`<config>` / `<data>` / `<state>` / `<logs>` / `<cache>` の実体は
次のとおりです。

| クラス     | Linux (XDG)                  | macOS                                     | Windows                         |
| :--------- | :--------------------------- | :---------------------------------------- | :------------------------------ |
| 設定       | `$XDG_CONFIG_HOME/flexagent` | `~/Library/Application Support/flexagent` | `%APPDATA%\flexagent`           |
| データ     | `$XDG_DATA_HOME/flexagent`   | `~/Library/Application Support/flexagent` | `%LOCALAPPDATA%\flexagent`      |
| 実行時     | `$XDG_STATE_HOME/flexagent`  | `~/Library/Application Support/flexagent` | `%LOCALAPPDATA%\flexagent`      |
| ログ       | `$XDG_STATE_HOME/flexagent`  | `~/Library/Logs/flexagent`                | `%LOCALAPPDATA%\flexagent\logs` |
| キャッシュ | `$XDG_CACHE_HOME/flexagent`  | `~/Library/Caches/flexagent`              | `%LOCALAPPDATA%\flexagent`      |

Linux では XDG にログ用のクラスがないため、`<logs>` は `<state>` と同じ
ディレクトリになります。`FXG_HOME` を指定した場合は、OS ルートの代わりに
`$FXG_HOME/{config,data,state,logs,cache}` を使います（portable モード）。

```text
<config>/
├── node.toml                       # ノードの起動設定のみ
└── hub.toml                        # ハブの起動設定のみ

<data>/
├── node/
│   ├── node.db                     # ノード所有の設定 + セッション正データ
│   ├── auth_token                  # ノード専用の Web アクセストークン (0600)
│   ├── node_token                  # ハブ接続用のペアリングトークン (0600)
│   ├── vapid.json                  # ノード単体運用向け Web Push VAPID 秘密鍵 (0600)
│   ├── secrets/<secret-id>         # ノード所有の秘密値 (0600)
│   └── worktrees/<project>/<branch>/  # 固定の Worktree 作成先
└── hub/
    ├── hub.db                      # ハブ所有の設定 + プロジェクト投影
    ├── auth_token                  # ハブ専用の Web アクセストークン (0600)
    ├── vapid.json                  # Web Push VAPID 秘密鍵 (0600)
    └── secrets/<secret-id>         # ハブ所有の秘密値 (0600)

<state>/
├── node/
│   └── node.addr                   # 稼働中ノードの待受アドレス (実行時のみ)
└── hub/
    └── hub.addr                    # 稼働中ハブの待受アドレス (実行時のみ)

<logs>/
├── node/node.log
└── hub/hub.log

<cache>/node/registry.json  # ACP Registry キャッシュ
```

| ファイル / ディレクトリ      | 用途                                                                           | 所有者・設定方法                                             |
| :--------------------------- | :----------------------------------------------------------------------------- | :----------------------------------------------------------- |
| `node.toml` / `hub.toml`     | 待受アドレス、ログ、`allowed_hosts` など起動時だけ必要な設定                   | 手動編集 + 再起動                                            |
| `<data>/<role>/auth_token`   | Web UI / CLI が **そのロールの** API にログインするための Web アクセストークン | 初回起動時に自動生成。`fxg node access-token show`（ノード） |
| `<data>/node/node_token`     | ノードが hub に接続するための **ノードペアリングトークン**                     | `fxg hub pairing-token issue` → Web UI / `fxg node pair`     |
| `<data>/<role>/<role>.db`    | 実行時設定、セッション、監査ログ、プロジェクト投影                             | Client API / Web UI                                          |
| `<state>/<role>/<role>.addr` | 起動中プロセスの実際の待受アドレス                                             | 実行時のみ。削除しても再生成される                           |
| `<data>/<role>/vapid.json`   | Web Push の VAPID 鍵ペア                                                       | 各ロールが初回起動時に生成                                   |
| `<data>/<role>/secrets/<id>` | エージェント・Git などの秘密値（書き込み専用、`0600`）                         | Secret API                                                   |
| `<data>/node/worktrees/`     | fxg が作成した Git Worktree                                                    | `fxg session new -w` / `fork -w`                             |
| `<cache>/node/`              | ACP Registry のキャッシュ                                                      | 自動                                                         |
| `<logs>/<role>/<role>.log`   | プロセスのログ                                                                 | 自動                                                         |

### 2.1.1 どのトークンを使うか

「token」は用途ごとに名前を分けています。接続できないときは、まずこの表で
どのトークンが必要かを確認してください。

| 呼び名                       | ファイル / 環境変数          | 何のため                                                            | 入手・設定方法                                                                                                                        |
| :--------------------------- | :--------------------------- | :------------------------------------------------------------------ | :------------------------------------------------------------------------------------------------------------------------------------ |
| **Web アクセストークン**     | `<data>/<role>/auth_token`   | ブラウザや CLI が **そのロールの** hub / node の API にログインする | 自動生成。ノードは `fxg node access-token show`、hub は hub ホストの `<data>/hub/auth_token`                                          |
| **ノードペアリングトークン** | `<data>/node/node_token`     | **ノードを hub に登録する**（hub ⇔ node 間）                        | hub で `fxg hub pairing-token issue <node-id>` → ノードの Web UI（設定 > ハブ接続）または `fxg node pair --hub <URL> --token <TOKEN>` |
| その他の秘密                 | `<data>/<role>/secrets/<id>` | エージェントの API キー、Git 認証など                               | Secret API（値は書き込み専用）                                                                                                        |

- Web アクセストークンはブラウザの Cookie (`fxg_session`) にも使われます。
  ノードとハブでファイルが分かれるため、片方のトークンでもう片方には
  ログインできません。
- ノードペアリングトークンは hub の `nodes.token_hash` にハッシュだけが保存され、
  発行時に一度だけ表示されます。
- ペアリングはノードが所有します。接続先は `node.db` (`node_pairing`)、トークンは
  `<data>/node/node_token` (`0600`) に保存し、`node.toml` は
  書き換えません。
- Web UI の **設定 > ハブ接続** から設定・接続テスト・解除ができます。変更は
  ノード API 経由で即時反映され、`fxg node restart` は不要です。

### 2.2 `node.toml` / `hub.toml` スキーマ（起動設定のみ）

```toml
# <config>/node.toml
node_id = "home-win"
listen_addr = "127.0.0.1:7860"
# ループバック以外のホスト名（リバースプロキシ / node 単体運用）で公開する場合のみ
allowed_hosts = ["home-win.tailnet-xxxx.ts.net"]
allowed_origins = ["https://home-win.tailnet-xxxx.ts.net"]

[log]
level = "info"
file = "node.log"      # 相対パスは <logs>/node/ 基準
format = "text"
no_color = false
```

```toml
# <config>/hub.toml
listen_addr = "0.0.0.0:8080"
allowed_hosts = ["home-server.tailnet-xxxx.ts.net:8080"]
allowed_origins = ["https://home-server.tailnet-xxxx.ts.net"]

[log]
level = "info"
file = "hub.log"       # 相対パスは <logs>/hub/ 基準
format = "text"
no_color = false
```

`node.name` / `project_scan_dirs` / `tray` / `vapid_subject`、
`[agents]`、ハブ接続先は `node.db` で、`[provisioners]` とハブ用 `vapid_subject` は
`hub.db` で管理します。`node.toml` / `hub.toml` は起動設定のみを持ち、未知のキーを拒否するため、
タイポが残っている場合は起動時に解析エラーになります。
Git 認証は Provisioner の `env_secrets` で注入します。Web UI の設定カタログは
現在値と出所 (`node.toml` / `hub.toml` / `env` / `cli` / `default`) を
読み取り専用で表示します。

`allowed_hosts` / `allowed_origins` は、Host ヘッダ検証（DNS Rebinding 対策）と
WebSocket の `Origin` 検証（CSWSH 対策）の許可リストです（ハブは `hub.toml`、
ノードは `node.toml`）。いずれも**完全一致**で判定し、既定は空（ループバック /
同一オリジンのみ許可）です。ブラウザは既定ポート以外ではポート番号を付ける
ため、`:8443` のような非標準ポートで公開する場合はポート込みで記載してください。
`allowed_origins` は `https://host` 形式（スキーム必須）です。これらは起動時に
読み込まれ、変更にはプロセス再起動が必要です（Web UI は読み取り専用）。

`node.toml` の `listen_addr` の既定は `127.0.0.1:7860` です。7860 が使用中で
明示指定がない場合は ephemeral port にフォールバックし、実際のアドレスは
`<state>/node/node.addr` に記録されます。CLI・`fxg node web`・
ブラウザのローカル直結はこのファイルを優先します。ワイルドカード
（`0.0.0.0` / `[::]`）で bind した場合、CLI と `fxg node web` は `127.0.0.1` に
読み替えて接続します。

### 2.3 Worktree と 一時VM環境

- Worktree の作成先は `<data>/node/worktrees/{project}/{branch}` に固定です。
  Worktree の作成は Web UI、または `fxg session new <agent> -w <branch> [--base <branch>]`
  / `fxg session fork <id> -w <branch>` から行います。
- 一時VMの環境構築・ツールセットアップは、コンテナイメージやVMテンプレート、またはプロビジョナーのコマンド定義で外部管理されます。`fxg` 自身はスクリプト合成やツール自動検出を行わず、`fxg internal stdio` は与えられたワークスペース上でプロトコル通信に専念します。

### 2.4 環境変数を使ってよい範囲

環境変数はプロセス起動時の入力であり、実行時に変更する設定の保存先にはしません。
`FXG_*` を追加してよいのは次の 3 種類だけです。

- **起動前 bootstrap**: DB を開く前に確定する値。`FXG_HOME`、`FXG_NODE_*`、
  `FXG_HUB_*`、`FXG_LOG_*` が該当し、`node.toml` / `hub.toml` と同じ設定を
  上書きします（優先順位は CLI フラグ > 環境変数 > toml > 既定値）。
- **エコシステム標準**: `RUST_LOG` / `NO_COLOR` / `CLICOLOR_FORCE`。fxg 固有の
  設定ではなく、他のツールと同じ意味で扱います。
- **テスト専用フック**: `cfg(test)` などで本番ビルドから隔離されているもの。

エージェントのチューニングや外部サービスへの接続先など、実行中に変わり得る値は
所有者の DB（`node.db` / `hub.db`）に置き、Client API から変更します。新しい
`FXG_*` を追加するときは `crates/fxg-protocol/src/config.rs` の `env_keys` に
定義し、settings API の `source` 表示に載せてください。

---

Web UI / Android PWA の設計は [`07-web-ui-and-pwa.md`](./07-web-ui-and-pwa.md) を参照してください。
