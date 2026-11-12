# 05. CLI (`fxg`) 設計・設定ファイルスキーマ・PWA フロントエンド設計

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
- **対象の指定**: フラグではなく位置引数（`fxg web hub`、`fxg logs hub`、
  `fxg service install hub`）。
- **緊急停止**: 全ノード一斉の kill switch はハブの Web UI /
  `POST /api/v1/system/kill-switch` に置く。ローカルのノードは
  `fxg node stop` で停止する。

### 1.2 コマンド群

- `fxg` / `fxg web [node|hub]`: トークン付き URL で Web UI を開く。
- `fxg session ...`: セッションの作成・一覧・表示・プロンプト・停止・再開・
  Fork・Import・Archive・Delete。
- `fxg node ...`: ローカルノードの起動・停止・状態・ペアリング・Web
  アクセストークン。
- `fxg hub ...`: ハブの起動・停止・状態・ノードペアリングトークン。
- `fxg service ...`: OS ログイン時の自動起動登録。
- `fxg logs` / `fxg doctor` / `fxg support`: ログ・診断・サポートバンドル。
- `fxg agents login|logout`: ACP エージェントの認証。
- `fxg internal ...`: システム連携用の隠しコマンド。

`fxg session ...` などのローカル API を使うコマンドは、ノードが起動していない
場合に `fxg node start --detach` 相当で自動起動します。自動起動を無効化するには
`FXG_NO_AUTO_NODE=1` を設定します。

## 2. 設定の所有者とディレクトリ構造

設定は「その設定を直接使うプロセス」が唯一の所有者です。実行時に変更する設定は
所有者の DB に型付き構造で保存し、`config.toml` にはプロセス起動時に必要な値だけを
残します。

| 設定                                                            | 所有者 / 保存場所                                                | 変更手段                                          |
| :-------------------------------------------------------------- | :--------------------------------------------------------------- | :------------------------------------------------ |
| 待受アドレス・ログ・`allowed_hosts` / `allowed_origins`         | `config.toml` / `FXG_*` / CLI                                    | ファイル編集 + 再起動（Web UI は読み取り専用）    |
| ハブ接続先・ペアリングトークン                                  | `node.db` (`node_pairing`) + `node_token` (`0600`)               | Client API / Web UI（ハブは対象ノードへプロキシ） |
| `node.name` / `allow_remote_pty` / `project_scan_dirs` / `tray` | `node.db` (`node_settings`)                                      | Client API（ハブは対象ノードへプロキシ）          |
| エージェント既定値・エイリアス・カスタム定義                    | `node.db` (`agent_settings` / `agent_aliases` / `custom_agents`) | Client API                                        |
| 明示的な `project_key`                                          | `node.db` (`project_path_links`)                                 | Client API / Web UI                               |
| VAPID subject・Provisioner                                      | `hub.db`                                                         | ハブ Web UI                                       |
| 秘密の値                                                        | `~/.flexagent/secrets/{node,hub}/<id>` (`0600`)                  | Secret API（書き込み専用）                        |
| 言語・テーマ・ピン留め                                          | ブラウザ `localStorage`                                          | Web UI                                            |

### 2.1 データディレクトリ構成 (`~/.flexagent/`)

```text
~/.flexagent/
├── config.toml                 # 起動設定のみ (Node / Hub / Log)
├── auth_token                  # Web アクセストークン (0600)
├── node_token                  # ノードペアリングトークン (0600)
├── vapid.json                  # Web Push VAPID 秘密鍵 (Hub用, 0600)
├── node.db                     # ノード所有の設定 + セッション正データ
├── node.addr                   # 稼働中ノードの待受アドレス (実行時のみ)
├── hub.db                      # ハブ所有の設定 + プロジェクト投影
├── hub.addr                    # 稼働中ハブの待受アドレス (実行時のみ)
├── cache/registry.json         # ACP Registry キャッシュ
├── logs/                       # node.log / hub.log
├── secrets/
│   ├── node/<secret-id>        # ノード所有の秘密値 (0600)
│   └── hub/<secret-id>         # ハブ所有の秘密値 (0600)
└── worktrees/<project>/<branch>/  # 固定の Worktree 作成先
```

| ファイル / ディレクトリ  | 用途                                                                          | 所有者・設定方法                                             |
| :----------------------- | :---------------------------------------------------------------------------- | :----------------------------------------------------------- |
| `config.toml`            | 待受アドレス、ログ、`allowed_hosts` など起動時だけ必要な設定                  | 手動編集 + 再起動                                            |
| `auth_token`             | Web UI / CLI が hub または node にログインするための **Web アクセストークン** | 初回起動時に自動生成。`fxg node access-token show`（ノード） |
| `node_token`             | ノードが hub に接続するための **ノードペアリングトークン**                    | `fxg hub pairing-token issue` → Web UI / `fxg node pair`     |
| `node.db` / `hub.db`     | 実行時設定、セッション、監査ログ、プロジェクト投影                            | Client API / Web UI                                          |
| `node.addr` / `hub.addr` | 起動中プロセスの実際の待受アドレス                                            | 実行時のみ。削除しても再生成される                           |
| `vapid.json`             | Web Push の VAPID 鍵ペア                                                      | Hub が生成                                                   |
| `secrets/{node,hub}/`    | エージェント・Git などの秘密値（書き込み専用、`0600`）                        | Secret API                                                   |
| `worktrees/`             | fxg が作成した Git Worktree                                                   | `fxg session new -w` / `fork -w`                             |
| `cache/`                 | ACP Registry のキャッシュ                                                     | 自動                                                         |
| `logs/`                  | `node.log` / `hub.log`                                                        | 自動                                                         |

### 2.1.1 どのトークンを使うか

「token」は用途ごとに名前を分けています。接続できないときは、まずこの表で
どのトークンが必要かを確認してください。

| 呼び名                       | ファイル / 環境変数       | 何のため                                                      | 入手・設定方法                                                                                                                        |
| :--------------------------- | :------------------------ | :------------------------------------------------------------ | :------------------------------------------------------------------------------------------------------------------------------------ |
| **Web アクセストークン**     | `auth_token`              | ブラウザや CLI が **1 つの** hub / node の API にログインする | 自動生成。ノードは `fxg node access-token show`、hub は hub ホストの `~/.flexagent/auth_token`                                        |
| **ノードペアリングトークン** | `node_token`              | **ノードを hub に登録する**（hub ⇔ node 間）                  | hub で `fxg hub pairing-token issue <node-id>` → ノードの Web UI（設定 > ハブ接続）または `fxg node pair --hub <URL> --token <TOKEN>` |
| その他の秘密                 | `secrets/{node,hub}/<id>` | エージェントの API キー、Git 認証など                         | Secret API（値は書き込み専用）                                                                                                        |

- Web アクセストークンはブラウザの Cookie (`fxg_session`) にも使われます。
- ノードペアリングトークンは hub の `nodes.token_hash` にハッシュだけが保存され、
  発行時に一度だけ表示されます。
- ペアリングはノードが所有します。接続先は `node.db` (`node_pairing`)、トークンは
  `~/.flexagent/node_token` (`0600`) に保存し、`config.toml` は書き換えません。
- Web UI の **設定 > ハブ接続** から設定・接続テスト・解除ができます。変更は
  ノード API 経由で即時反映され、`fxg node restart` は不要です。

### 2.2 `config.toml` スキーマ（起動設定のみ）

```toml
[node]
node_id = "home-win"
listen_addr = "127.0.0.1:7860"

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
`[provisioners]`、`vapid_subject`、`[node] hub_url` は `node.db` / `hub.db` で
管理します。`config.toml` は起動設定のみを持ち、未知のキーを拒否するため、
タイポが残っている場合は起動時に解析エラーになります
（[`changelog/2026-11-11-strict-config-schema.md`](changelog/2026-11-11-strict-config-schema.md)）。
Git 認証は Provisioner の `env_secrets` で注入します。Web UI の設定カタログは
現在値と出所 (`config.toml` / `env` / `cli` / `default`) を読み取り専用で表示します。

`[node] listen_addr` の既定は `127.0.0.1:7860` です。7860 が使用中で明示指定が
ない場合は ephemeral port にフォールバックし、実際のアドレスは
`~/.flexagent/node.addr` に記録されます。CLI・`fxg web`・ブラウザのローカル直結は
このファイルを優先します。

### 2.3 Worktree と 一時VM環境

- Worktree の作成先は `~/.flexagent/worktrees/{project}/{branch}` に固定です。
  Worktree の作成は Web UI、または `fxg session new <agent> -w <branch> [--base <branch>]`
  / `fxg session fork <id> -w <branch>` から行います。
- 一時VMの環境構築・ツールセットアップは、コンテナイメージやVMテンプレート、またはプロビジョナーのコマンド定義で外部管理されます。`fxg` 自身はスクリプト合成やツール自動検出を行わず、`fxg internal stdio` は与えられたワークスペース上でプロトコル通信に専念します。

## 3. Web UI / Android PWA (`ui/`) 設計

PCブラウザ、ローカルフォールバック (`node.addr`、既定は `localhost:7860`)、および
Android スマートフォン（ホーム画面追加PWA）のすべてを単一のレスポンシブSPAで提供します。

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
- **Runes のリアクティブ境界 (Map / Set は Proxy 化されない)**:
  `$state` が深く Proxy 化するのは plain object と配列だけで、`Map` /
  `Set` を `$state` に入れても `set()` / `add()` は再描画を起こさない。
  時間とともに書き換わるコレクションは `svelte/reactivity` の `SvelteMap` /
  `SvelteSet` を使うか、更新のたびに値ごと再代入する。ストリーミングバッファ
  (`ui/src/lib/sync/timeline.svelte.ts` の `messageDeltas` / `thoughtDeltas`
  / `toolProgress` / `terminalDeltas`、`SyncStore` の `pendingPrompts` /
  `provisioningLogs`) は `SvelteMap` で保持する。この境界を破ると
  「最初のチャンクだけ表示され、確定イベントで全文が一気に出る」ため、
  `ui/src/lib/sync/timeline-reactivity.test.svelte.ts` で回帰を防ぐ。
- **コード・Diff・双方向ターミナル**:
  - **Diff / コード表示**:
    - PC: `monaco-editor` (Monaco Diff Editor: Side-by-side / Inline
      切替、ミニマップ、構文ハイライト。Svelte の `$effect` でマウント)
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
     - **「＋一時VMをスポーンして起動（例: Local Docker / Local Incus VM / Google
       Colab Pro）」**。リポジトリの clone やツールのセットアップは、プロビジョナーの
       起動コマンド / コンテナイメージ / VM テンプレート側で行います。fxg は
       `FXG_GIT_URL` / `FXG_GIT_BRANCH` などの環境変数を渡しますが、clone や
       `mise` / `uv` の自動導入は行いません。
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
       - 一時VM起動時（`provisioning`
         状態）は、最上部に折りたたみ式の **「Provisioner
         Log」カード** を表示し、VM起動やコンテナ初期化の
         `stderr` 出力をリアルタイム表示。
       - ストリームタイムライン（ユーザー発言、思考プロセス、ツール実行、承認カード、Pending
         Queue）。`LiveStreamDelta` は届いた順に逐次描画し、確定イベントで
         同じ位置の本文へ置き換える（描画はチャンク単位で合流させ、トークン
         単位の delta でも markdown 再パースが過剰にならないようにする）。
       - 思考プロセスはストリーミング中だけ自動展開し、確定したら自動で
         折りたたむ（読み手が明示的に開閉した場合はその意思を優先）。折りたたみ
         時はヘッダーに経過時間と先頭行のプレビューを表示し、展開中の本文は
         高さを抑えて内側だけ末尾追従する。
       - 出力の追従スクロールは「末尾付近を読んでいるとき」だけ行い、上に
         スクロールして過去を読んでいる間は新しい出力で位置を奪わない
         （自分の発言を送信したときは末尾へ戻る）。
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
     - 新規セッション画面とセッション詳細画面は同一のコンポーザーを使い、
       「文脈行（新規のみ: プロジェクト / 作業ディレクトリ / 環境）→
       入力 → セレクタ行（Agent / Mode / Model / その他の設定）→
       アクション行（添付 / 自動承認 / その他 / 中断 / 送信）」の順を固定する。
       項目が存在しない場合は非表示にするだけで、他の項目の位置は変えない。
     - セレクタ行は横スクロールし、アクション行は折り返さない。これにより
       ボタンが増えても送信ボタンの位置が動かない。
     - スラッシュコマンド補完、モード切替 (`plan` /
       `code`)、モデル選択、中断ボタン。
     - 添付は新規セッションでも送信できる。自動承認と Compact は開始後の
       セッションにのみ存在するため、新規セッションでは表示しない。
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
     ボタン。タップ時に確認モーダルを表示し、全ノードで稼働中の全セッション・プロセスツリー・PTYを即時停止し、
     一時VMは drain 後に破棄します。
   - 初回アクセス時または未認証時に表示される
     **「認証トークン入力ダイアログ」**（入力成功時に `fxg_session` Cookie
     を自動保持）。
   - ノード側でリモートPTYが無効化（`allow_remote_pty = false`）されている場合、Terminalペインに「リモートPTYはセキュリティポリシーにより無効化されています。ローカル端末（`localhost:7860`
     または CLI）からご利用ください」と安全にフォールバック表示。
6. **設定画面（スコープ + タブ構成）**:
   設定は所有者ごとに **Hub** / **Node** / **Browser** の3スコープに分かれ、
   スコープごとにタブが切り替わります。
   - **Hub**（ハブ接続時のみ）:
     - **Process**: `config.toml` の起動設定（読み取り専用）と診断情報。
     - **General**: ハブ設定 (VAPID subject) とクライアント認証トークンの再生成。
     - **Pairing tokens**: ノード個別トークンの発行・失効・一覧。
     - **Provisioners**: 一時VM・サンドボックスのプロビジョナー定義の追加・編集・削除と接続テスト。
     - **Audit log**: ハブ経由操作の監査ログ。
   - **Node**（ローカルノード接続時は自分のノード、ハブ接続時はオンラインの対象ノード）:
     - **Process**（ローカルノード接続時のみ）: 起動設定と診断情報。
     - **General**: 表示名 / `allow_remote_pty` / `project_scan_dirs` / `tray` などの
       ノード所有設定。ローカルノード接続時はクライアント認証トークンの再生成も可能。
     - **Hub pairing**: ハブ URL とペアリングトークンの入力、接続テスト、
       ペアリング / 解除、接続状態（未設定 / 接続中 / 接続済み / 認証エラー /
       接続失敗）と失敗理由の表示、自分の `node_id` のコピー。接続先を変える場合は
       「全履歴を再送し、変更前のハブのデータは残る」旨を表示する。
     - **Agents**: ノードで利用可能なエージェント (builtin の native CLI /
       カスタム定義) を一覧表示し、カスタム ACP エージェントの追加・編集・削除を行う。
       builtin にないエージェントは「Import from registry」から ACP Registry の
       配布レシピを custom 定義として取り込める。binary は `PATH`、npx/uvx は
       パッケージランナーを使ってコマンドを生成し、import 後は通常の custom として
       編集・削除できる。
     - **Audit log**（ローカルノード接続時のみ）: ローカル直結操作の監査ログ。
   - **Browser**:
     - **Appearance**: テーマ（システム / ライト / ダーク）、パレット、
       送信ショートカット、ピン留めの管理。

### 3.3 Android PWA & Web Push (VAPID) の実装詳細

- **`manifest.webmanifest`**: `"display": "standalone"`,
  `"theme_color": "#f7f5f0"`
  を設定し、Androidでネイティブアプリ同等のフルスクリーン起動を実現。
- **Service Worker (`ui/src/service-worker/index.ts` / `$app/manifest`)**:
  1. **App Shell キャッシュ**: SvelteKit 標準の `$app/manifest`（`assets` /
     `immutable`）と `$app/env`（`version`）を用いてHTML/JS/CSSをキャッシュし、モバイル回線が不安定な場所やハブ障害時でもUIが即座に立ち上がるようにします。
     ナビゲーションはネットワーク優先 (オフライン時のみ `index.html`
     へフォールバック) で、
     `/api/**` は常にネットワークへ素通しします。
  2. **Web Push 受信 & アクションボタン**: ハブからVAPID Web
     Pushを受信した際、Android通知バナーに以下を表示します：
     - タイトル: `[flexagent] Approval request (<node>) — <session title>`
     - 本文: `<tool name>: <summary>`
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
     `[flexagent] Question (<node>) — <session title>` として通知します。form の回答はアプリ内
     でしか行えないため、承認のような `[Approve]` / `[Reject]`
     ボタンは付けず、タップでアプリのセッション / Inbox を開きます
     (`allow_option_id` / `reject_option_id` は `null`)。

### 3.4 統合 Web ターミナルと抽象化設計 (`ITerminalAdapter`)

Webターミナルは、GhosttyのネイティブVTエミュレーション精度とCanvas
2D高速描画を持つ **`ghostty-web`**
を標準採用します。同時に、UIコンポーネントと特定のターミナル実装を疎結合にするため、薄い抽象化インターフェースを介して利用します。

インターフェースの正は `ui/src/lib/components/terminal/types.ts` です。
`mount` / `clear` / `write` / `onData` / `onResize` / `fit` / `focus` /
`dispose` を持ちます。

- **`GhosttyWebAdapter` (標準実装)**:
  - `ghostty-web` をラップ。
  - WASMによる高速なANSI/VT100シーケンス解釈とCanvas
    2D描画により、ビルドログ等の大量出力でもUIスレッドをブロックしません。
- **差し替え容易性 (Pluggable)**:
  - 将来的にAndroid
    PWAでのネイティブ文字選択・IME入力の操作性やアクセシビリティを重視したいケースが生じた場合でも、DOMレンダラを採用する
    `wterm` (`@wterm/ghostty`) 等のアダプタへ最小限の修正で切り替え可能です。
- **Svelte コンポーネント (`TerminalView.svelte`)**:
  - `onMount` と `FitAddon.observeResize()` による自動 `fit()` 実行と、
    ノード側PTYへの `resize` メッセージ送信。
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
- [ ] ローカルループバック HTTP REST / WS (`127.0.0.1:7860` 既定 / 衝突時は
      ephemeral port) による `fxg` CLI ⇔ `fxg node` 疎通
- [ ] **セキュリティ基礎**:
  - [ ] `fxg node` ローカルAPIの `127.0.0.1`（ループバック、既定ポート 7860）
        厳格バインドと `node.addr` の公開
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
- [x] Service Worker (`ui/src/service-worker/index.ts`) と `web-push-native`
      クレートによる
      VAPID Push通知（Androidバックグラウンド通知＆バナー承認）の実装
- [x] `fxg service install` による各OS自動起動設定の実装

### Milestone 6: 一時VM・サンドボックスノード (`fxg internal stdio` & Provisioner) (実装計画 Phase 6 対応)

- [x] `fxg internal stdio` の JSON Lines 通信と `stderr` 分離
- [x] `fxg hub` のコマンドテンプレート型プロビジョナー管理
  - `{INSTANCE_NAME}` / `{SESSION_ID}` / `{NODE_ID}` を置換し、プロビジョナー定義の
    環境変数を注入する。
  - `stderr` は `HubWsMessage::ProvisioningLog` として UI へエフェメラル配信する。
  - 定義は Web UI の **Hub > Provisioners** タブで追加・編集・削除・接続テストできる。
- [x] `DrainAndShutdown` による一時VM破棄前の未送信イベント完全フラッシュ
  - アイドルタイムアウト (`idle_timeout_secs`) またはセッション終了で Drain し、
    未送信イベントを送信してから `DrainComplete` を返す。Drain
    前クラッシュ時の中間イベント損失は許容仕様。
- [x] ツールチェインとリポジトリ配置は Dockerfile / VM テンプレート / 起動コマンド側で管理
- [x] Git 認証は Provisioner の `env_secrets` とイメージ側の credential helper で完結
