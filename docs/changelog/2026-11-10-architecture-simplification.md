# アーキテクチャの単純化（通信経路のHTTP一本化・Git Askpass撤去・Provisioner環境構築の外部化）

- **日付**: 2026-11-10
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-node`, `fxg-hub`, `fxg-cli`, `ui`, `docs`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run check`, `mise run lint`, `cargo test --workspace`

---

## 1. 背景と課題

これまで FlexAgent の実装・運用を進める中で、主に以下の3点がシステム全体の複雑さを増幅させている根本課題として浮上した。

1. **ソケット (Local IPC) と HTTP という2つの経路の二重化**:
   - `fxg node start` はローカル HTTP/WS サーバー (`127.0.0.1:7860`) とローカル IPC サーバー（Unix Domain Socket / Windows Named Pipe）を両方起動していた。
   - CLI (`fxg session ...`) は専用の Length-prefixed JSON IPC プロトコル (`IpcRequest`, `LocalRequest` 等、約2,000行) でノードと通信しており、同一の操作が REST と IPC の双方に二重実装されていた。
   - Windows Named Pipe の接続競合 (`ERROR_PIPE_BUSY`) や Unix Domain Socket の残留ファイル削除など、OS 固有のソケット管理がプラットフォーム間の複雑さを招いていた。
2. **Git Askpass の過剰な密結合**:
   - 一時 VM のディスクや環境変数にトークンを残さないという過剰なステートレス志向のため、「`git` ➔ `git-askpass.sh` ➔ Local IPC ➔ `CredentialBroker` ➔ Hub WS (`GitCredentialRequest`) ➔ Hub」という多段バケツリレーが構築されていた。
   - 一時 VM (`fxg internal stdio`) が Local IPC サーバーを起動している唯一の理由がこの `git-askpass` のためだった。
   - さらに、常駐ノード（ローカルマシン）でもエージェント起動時に無条件で `GIT_ASKPASS` を注入しており、開発者自身の既存の Git 認証（SSH 鍵、Git Credential Manager 等）を邪魔する原因になっていた。
3. **Provisioner の仕組みの複雑化（環境セットアップの抱え込み）**:
   - Hub が `{BOOTSTRAP_SCRIPT}` として長大なシェルスクリプトを動的合成し、`curl` で `fxg` バイナリを落とし、`bootstrap.rs`（706行）がリポジトリを解析して `mise` / `uv` / 言語ツールチェインを都度ダウンロード・インストールし、`project_profiles` のカスタムスクリプトを実行していた。
   - ツールチェイン導入に時間がかかるだけでなく、環境構築失敗のデバッグが難しく、本来 Dockerfile や VM テンプレート側で用意すべき責務を `fxg` が抱え込んでいた。

---

## 2. 方針と設計決定

### 2.1 通信経路の一本化 (Local IPC 廃止・HTTP 統一)

- **Local IPC (Named Pipe / Unix Domain Socket) を全廃**:
  - `crates/fxg-protocol/src/ipc.rs`、`crates/fxg-node/src/daemon/ipc.rs`、`ipc_client.rs`、`ipc_framing.rs` を削除する。
- **CLI ⇔ Node 通信を `http://127.0.0.1:7860` の REST API に統一**:
  - `fxg node start` はローカルループバック限定の HTTP/WS サーバーのみを起動し、起動アドレスは `~/.flexagent/node.addr`、認証トークンは `~/.flexagent/auth_token` (`0600`) から読み取る。
  - CLI の `NodeClient` は `HubClient` と同様の `reqwest` ベースの HTTP クライアントに再実装する。
  - ノードの自動起動待機も、`GET /healthz` のポーリングに統一する。
- **REST エンドポイントの追加・完全共通化**:
  - `LocalRequest` に残っていた数個の操作（`prompt`, `control`, `show`, `import` 等）を REST エンドポイント (`client_router`) に追加する。
  - これにより、CLI・Web UI・外部ツールが 100% 同一の REST / WebSocket API を共有する。

### 2.2 Git Askpass / 認証プロキシの完全削除

- **`askpass` コマンドおよび動的 RPC リレーの完全撤去**:
  - `crates/fxg-node/src/credentials.rs`、`crates/fxg-hub/src/credentials.rs`、`fxg internal git-askpass` サブコマンド、および `GitCredentialRequest` / `GitCredentialResponse` メッセージを全廃する。
  - `hub.db` の `git_credentials` テーブルおよび関連 API / UI を廃止する。
- **Git 認証は外部環境変数注入に委ねる**:
  - ローカル常駐ノードでは `GIT_ASKPASS` を一切注入せず、開発者の既存の認証環境（SSH 鍵、Git Credential Manager、`gh auth` 等）を尊重する。
  - 一時 VM / コンテナ環境では、必要に応じて Provisioner の `env` に `GH_TOKEN` や `GITHUB_TOKEN` 等を注入するか、コンテナ内で `gh auth setup-git` や SSH agent を外部から設定する。`fxg` 自身は Git 認証の仲介を一切行わない。

### 2.3 Provisioner の軽量化 (環境セットアップの外部化)

- **`bootstrap-workspace`・動的ツールインストーラの全廃**:
  - `crates/fxg-node/src/bootstrap.rs`（706行）および `fxg internal bootstrap-workspace` を完全削除する。
  - `hub.db` の `project_profiles` テーブルおよび関連 API / UI を廃止する。
  - 言語ツールチェイン（Rust, Node, Python 等）やパッケージのセットアップは、コンテナイメージ（Dockerfile）や VM テンプレート、Provisioner の起動スクリプトなど、外部環境側で事前定義する。
- **スクリプト合成の廃止とシンプルな子プロセス起動**:
  - Hub の `compose_bootstrap_script` を廃止する。
  - Provisioner 定義は「実行コマンド (`command`)」「引数 (`args`)」「環境変数 (`env`)」「タイムアウト」のみとし、Hub は素直にそのコマンドを実行して `stdin` / `stdout` パイプを繋ぎ、`fxg internal stdio` とプロトコル通信を行う。
  - 一時ノードでのリポジトリ取得が必要な場合は、コンテナ側で用意するか、`fxg internal stdio --clone <url> --workspace <dir>` のような最小限の Git clone のみに単純化する。

---

## 3. コード削減規模の見込み

| 領域                   | 削除・整理対象                                                                             |   削減行数 (目安)   |
| :--------------------- | :----------------------------------------------------------------------------------------- | :-----------------: |
| **Local IPC 廃止**     | `fxg-protocol/src/ipc.rs`, `fxg-node/src/daemon/ipc.rs`, `ipc_client.rs`, `ipc_framing.rs` |     約 2,000 行     |
| **Git Askpass 撤去**   | `fxg-node/src/credentials.rs`, `fxg-hub/src/credentials.rs`, `git_credentials`             |      約 550 行      |
| **Provisioner 軽量化** | `fxg-node/src/bootstrap.rs`, `compose_bootstrap_script`, `project_profiles`                |      約 950 行      |
| **合計**               | 不要なプロトコル・IPC・認証プロキシ・自動ツールインストーラが消滅                          | **約 3,500 行以上** |

---

## 4. 段階的実装計画 (Checklist)

- [ ] **Phase 1: Git Askpass / 認証プロキシの完全削除**
  - [ ] `NodeToHubMsg::GitCredentialRequest` / `HubToNodeMsg::GitCredentialResponse` の削除
  - [ ] `fxg-node/src/credentials.rs` の削除
  - [ ] `fxg-hub/src/credentials.rs` および `hub.rs` 内の relay 処理削除
  - [ ] `SessionManager` での `GIT_ASKPASS` 注入処理の削除
  - [ ] `fxg internal git-askpass` コマンドの削除
  - [ ] `hub.db` の `git_credentials` テーブル・API・UI の削除
  - [ ] `stdio` モード (`fxg internal stdio`) から IPC サーバー起動処理を削除
- [ ] **Phase 2: 通信経路の HTTP 一本化 (Local IPC 廃止)**
  - [ ] `client_router` に不足している REST エンドポイントを追加 (`GET /api/v1/sessions/:id`, `POST /api/v1/sessions/:id/prompt`, `POST /api/v1/sessions/:id/control`, `POST /api/v1/sessions/import`)
  - [ ] `fxg-cli/src/client.rs` を HTTP クライアント (`reqwest`) に全面移行
  - [ ] `fxg node start` から Local IPC サーバー（UDS / Named Pipe）起動処理を削除
  - [ ] `fxg-protocol/src/ipc.rs`, `fxg-node/src/daemon/ipc.rs`, `ipc_client.rs`, `ipc_framing.rs` を削除
- [ ] **Phase 3: Provisioner の軽量化 (環境セットアップ外部化)**
  - [ ] `fxg-node/src/bootstrap.rs` および `fxg internal bootstrap-workspace` の削除
  - [ ] `compose_bootstrap_script` による長大スクリプト合成の廃止
  - [ ] `hub.db` の `project_profiles` テーブル・API・UI の削除
  - [ ] 必要に応じた `fxg internal stdio` へのシンプルな `--clone <url>` オプション追加
- [ ] **Phase 4: ドキュメント・型定義・テストの同期**
  - [ ] `docs/` 各ファイルの整合性確認
  - [ ] `mise run types` でフロントエンド型同期
  - [ ] `cargo test --workspace` でテスト通過を確認
