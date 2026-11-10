# アーキテクチャの単純化（通信経路のHTTP一本化・Git Askpass撤去・Provisioner環境構築の外部化）

- **日付**: 2026-11-10
- **状態**: 実装済み（PR #6）
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-node`, `fxg-hub`, `fxg-cli`, `ui`, `docs`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run lint:errors`, `mise run types`, `cargo test --workspace`

> 本文は目標アーキテクチャです。実装は1つのPR内の順序付きコミットとして行いました。
> 現在のコードと `docs/` の現行仕様は、この目標状態に一致しています。

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

## 2. 確定した設計決定

### 2.0 決定サマリ

| ID  | 決定                                                                                                      |
| :-- | :-------------------------------------------------------------------------------------------------------- |
| D1  | フェーズ分割リリースは行わず、**1つの実装PR内の順序付きコミット**として一括実施する。                     |
| D2  | 一時VM/コンテナ環境は**完全外部化**する。Hub は `fxg` の導入・clone・ツールチェイン導入を行わない。       |
| D3  | Git 認証は Provisioner の **`env_secrets`** による secret 注入に統一し、askpass の RPC リレーを全廃する。 |
| D4  | CLI は **REST 追加 + `/api/v1/client/ws` 購読**に移行する。                                               |
| D5  | 既存の `{BOOTSTRAP_SCRIPT}` 定義には**互換シムを提供しない**。置換も検出も行わない。                      |
| D6  | 本計画PRの docs は**目標状態 + 「計画中（未実装）」バナー**とし、`AGENTS.md` は実装まで現状維持する。     |
| D7  | `PROTOCOL_VERSION` を **2** に上げ、migration `0012` で不要テーブルを DROP する。旧ノードは明示エラー。   |
| D8  | ローカル HTTP は既定 **`127.0.0.1:7860`**。`AddrInUse` 時のみ **ephemeral port** にフォールバックする。   |
| D9  | `Bootstrapping` を廃止して `Provisioning` に集約し、`BootstrapLog` を `ProvisioningLog` に改名する。      |

### 2.1 通信経路の一本化 (Local IPC 廃止・HTTP 統一)

- **Local IPC (Named Pipe / Unix Domain Socket) を全廃**:
  - `crates/fxg-protocol/src/ipc.rs`、`crates/fxg-node/src/daemon/ipc.rs`、`ipc_client.rs`、`ipc_framing.rs` を削除する。
  - 付随して `DaemonConfig.ipc_endpoint`、`FXG_IPC_ENDPOINT`、`paths::ipc_endpoint`、テスト用 IPC エンドポイント、CLI の IPC 表示を削除する。
- **CLI ⇔ Node 通信をループバック HTTP REST / WebSocket に統一**:
  - `fxg node start` はローカルループバック限定の HTTP/WS サーバーのみを起動する。
  - 待受アドレスの既定は `127.0.0.1:7860`。`AddrInUse` かつ `[node] listen_addr` / `--listen` が明示されていない場合のみ `127.0.0.1:0`（ephemeral port）で再 bind し、実際のアドレスを `node.addr` に temp + rename で書く。明示指定時の bind 失敗はフォールバックせずエラーにする。
  - CLI・`fxg web`・ブラウザのローカル直結は `node.addr` を優先する。stale addr は `GET /healthz` で無効化し、必要なら自動起動して addr を読み直す。
  - 認証は `~/.flexagent/auth_token` (`0600`) の Bearer トークン（または Cookie）を使う。`/healthz`・`/readyz`・`/api/v1/meta` は認証免除のまま。
- **REST エンドポイントの追加・完全共通化**:
  - 追加: `GET /api/v1/sessions/:id`、`POST /api/v1/sessions/:id/prompt`、`POST /api/v1/sessions/:id/control`、`POST /api/v1/sessions/import`。
  - 既存の `GET /api/v1/sessions/:id/events` に `limit` / `before_cursor` を追加し、`fxg session show` の末尾イベント取得に使う。
  - ローカルノードの `POST /api/v1/system/shutdown` を実装し、`fxg node stop` を IPC なしで動かす。
  - `POST /prompt` / `/control` は dispatch 後に `CommandResult` を返す（ターン完了は待たない）。`fxg session prompt --wait` は `/api/v1/client/ws` の `EventBatch` を購読して完了を判定する。`command_id` による冪等性は既存の `dispatch_command` と共通化する。
  - `fxg node status` は `GET /api/v1/system/info`（`hub_connected` / `unsynced_event_count`）と `GET /api/v1/sessions` を使う。`/api/v1/meta` は接続種別のみを返すため使わない。
  - `fxg session new` / `-w` は cwd から `POST /api/v1/nodes/:node_id/projects/inspect` で `project_id` を解決し、`POST /api/v1/sessions` の `local_path` / `worktree` を使う。独立した `fxg worktree` コマンドは存在しない。
  - `SessionDetail` は `ipc.rs` から `fxg-protocol::client_api` へ移設する。`LocalStatus` は CLI 側で `/system/info` + セッション一覧から再構成し、IPC 専用型として残さない。
  - CLI は `tokio-tungstenite` で `/api/v1/client/ws` に接続し、`Subscribe` / `EventBatch` / `LiveStreamDelta` / `CommandResult` を処理する。ブラウザ以外のクライアントは `Origin` を送らないため、既存の CSWSH 検証をそのまま通過する。

### 2.2 Git Askpass / 認証プロキシの完全削除

- **`askpass` コマンドおよび動的 RPC リレーの完全撤去**:
  - `crates/fxg-node/src/credentials.rs`、`crates/fxg-hub/src/credentials.rs`、`fxg internal git-askpass` サブコマンド、`NodeToHubMsg::GitCredentialRequest` / `HubToNodeMsg::GitCredentialResponse` を全廃する。
  - `hub.rs` の credential relay、`provisioner::resolve_git_token` の Hub 経由解決、`hub.db` の `git_credentials` テーブル・API・UI を削除する。
  - `SessionManager` の `GIT_ASKPASS` / `GIT_TERMINAL_PROMPT` 注入を削除する。ローカル常駐ノードは開発者自身の SSH 鍵・Git Credential Manager・`gh auth` をそのまま使う。
- **一時VM/コンテナは Provisioner の secret 注入に統一**:
  - `ProvisionerDefinition` に `env_secrets: BTreeMap<String, SecretId>` を追加する（env 名 → secret ID）。
  - `provisioners` テーブルに `env_secrets_json` を追加し、spawn 時に `~/.flexagent/secrets/hub/<id>` から値を解決して環境変数として注入する。平文トークンは DB に保存しない。
  - `secrets::used_by` の Hub 側は `git_credentials` ではなく Provisioner を走査し、使用中 secret の削除を防ぐ。
  - イメージ / VM / 起動コマンド側で `GH_TOKEN` 等を受け取り、`gh auth setup-git`・credential helper・SSH agent のいずれかで Git に渡す。`fxg` 自身は認証を仲介しない。
  - `git_credentials` に保存済みのトークンは移行されない。アップグレード前に secret として再登録する必要がある旨を docs に明記する。

### 2.3 Provisioner の軽量化 (環境セットアップの完全外部化)

- **`bootstrap-workspace`・動的ツールインストーラの全廃**:
  - `crates/fxg-node/src/bootstrap.rs`（706行）および `fxg internal bootstrap-workspace` を完全削除する。
  - `hub.db` の `project_profiles` テーブル・API・UI、`FXG_PROJECT_PROFILE_B64`、`bootstrap.env`、`BOOTSTRAP_ENV_FILE_NAME` を削除する。
  - 言語ツールチェイン（Rust, Node, Python 等）やパッケージのセットアップは、コンテナイメージ（Dockerfile）や VM テンプレート、Provisioner の起動コマンドで事前定義する。
- **スクリプト合成の廃止とシンプルな子プロセス起動**:
  - `compose_bootstrap_script`、`EPHEMERAL_BINARY`、`EPHEMERAL_WORKSPACE`、`FXG_BINARY_URL` を削除する。
  - Provisioner 定義は「実行コマンド (`command`)」「引数 (`args`)」「環境変数 (`env`)」「secret 環境変数 (`env_secrets`)」「タイムアウト」のみとし、Hub はそのコマンドを実行して `stdin` / `stdout` を繋ぎ、`fxg internal stdio` とプロトコル通信を行う。
  - Hub が注入する環境変数は `FXG_NODE_ID` / `FXG_NODE_NAME` / `FXG_GIT_URL` / `FXG_GIT_BRANCH` と、`env` / `env_secrets` の内容に限定する。
  - `fxg internal stdio --clone` は追加しない。リポジトリ取得は環境側の責務とする。
- **既存の `{BOOTSTRAP_SCRIPT}` 定義は互換なし**:
  - 置換も検出も行わない。既存定義には literal が渡り、docker / VM 側の引数エラーになる。
  - docs に「イメージ/起動コマンド側へ `fxg` 導入・clone・ツール導入を移す」移行手順と、Dockerfile / Colab の参照例を記載する。診断用の spawn 時検出は追加しない。
- **外部環境の契約（docs に明記）**:
  - `fxg`（`PROTOCOL_VERSION` と一致するバージョン）が導入済みであること。
  - `git` と必要なツールチェインが導入済みであること。
  - `FXG_GIT_URL` / `FXG_GIT_BRANCH` がある場合、リポジトリが workspace に配置済みであること。
  - コマンドは `fxg internal stdio --ephemeral --workspace <dir>` を実行し、`stdout` を JSON Lines 専用、ログを `stderr` に出すこと。

### 2.4 DB・プロトコル・命名

- **プロトコル版**:
  - `PROTOCOL_VERSION` を `1` から `2` に上げる。ハブは既存の `NodeHello` チェックで旧ノードを明示エラーにする。
  - CLI は `GET /api/v1/system/info` の `version` を確認し、古い常駐サービスなら `fxg node restart` を案内する。
- **migrations**:
  - `0012_provisioner_env_secrets.sql`: `provisioners.env_secrets_json` を ADD する（Commit 3）。
  - `0013_remove_project_profiles.sql`: `project_profiles` を DROP する（Commit 4）。
  - 後続 migration（Commit 5 / 7）: `git_credentials` を DROP し、`sessions.status` / `nodes.lifecycle_status` の `bootstrapping` を `provisioning` に、`session_events.event_type` の `bootstrap_log` を `provisioning_log` に更新する。
  - 既存の `payload_json` は `ProvisioningLog` の `#[serde(alias = "bootstrap_log")]` で読み取り、JSON の一括書き換えは行わない。
- **命名**:
  - `SessionStatus::Bootstrapping` / `NodeLifecycleStatus::Bootstrapping` を削除し、既存の `Provisioning` に集約する。
  - `UnifiedEventPayload::BootstrapLog` → `ProvisioningLog`、`EventType::BootstrapLog` → `ProvisioningLog`、`HubWsMessage::BootstrapLog` / `ClientEvent::BootstrapLog` → `ProvisioningLog` に改名する。保存済み `payload_json` を読むため、`ProvisioningLog` に `#[serde(alias = "bootstrap_log")]` を付ける。WS の旧クライアントは同一PRのUI更新後に再接続・再読み込みが必要になる。
  - UI の `status_bootstrapping` i18n、`node-status.ts` / `session-status.ts`、生成 bindings を更新する。
- **設定・型の整理**:
  - `DEFAULT_GIT_USERNAME`、`BOOTSTRAP_ENV_FILE_NAME`、`GitCredentialConfig` / `ProjectProfile` 関連の protocol 型・API・UI を削除する。
  - `mise run types` で `ui/src/lib/generated/bindings.ts` を同期する。

### 2.5 ドキュメント

- 本PRでは `docs/README.md` / `01` / `02` / `03` / `04` / `05` の冒頭に「計画中（未実装）」バナーを付ける。
- `AGENTS.md` は実装PRまで master の記述に戻す（計画段階で「廃止済み」と誤認させない）。
- 取り残しを修正する:
  - `docs/README.md` のドキュメント表・クレート構成から Git 認証プロキシ / ローカルIPC の記述を除去。
  - `docs/01` の sequence diagram から `GitCredentialRequest` の注記を除去し、環境側で clone / ツール導入を行う前提に合わせる。
  - `docs/04` の WebSocket/IPC 記述を WebSocket に統一。
  - `docs/03` の存在しない `FXG_NODE_URL`、`/api/v1/meta` による status 取得、存在しない `fxg worktree` / `fxg inbox`、`fxg status` の名称を修正し、新しい REST 契約を追記する。
  - `docs/05` の Milestone 6 は実装済み項目を `[ ]` に戻さず、「アーキテクチャ簡素化で廃止予定」と注記する。

---

## 3. コード削減規模の見込み

| 領域                   | 削除・整理対象                                                                             | 削除行数 (目安) |
| :--------------------- | :----------------------------------------------------------------------------------------- | :-------------: |
| **Local IPC 廃止**     | `fxg-protocol/src/ipc.rs`, `fxg-node/src/daemon/ipc.rs`, `ipc_client.rs`, `ipc_framing.rs` |   約 2,000 行   |
| **Git Askpass 撤去**   | `fxg-node/src/credentials.rs`, `fxg-hub/src/credentials.rs`, `git_credentials`             |    約 550 行    |
| **Provisioner 軽量化** | `fxg-node/src/bootstrap.rs`, `compose_bootstrap_script`, `project_profiles`                |    約 950 行    |
| **合計（グロス）**     | 不要なプロトコル・IPC・認証プロキシ・自動ツールインストーラが消滅                          | **約 3,500 行** |

- 純減はこれより小さい。`fxg-cli/src/client.rs`（約730行）は HTTP クライアントへ書き換え、REST 追加・WS クライアント・`env_secrets`・UI・テストで約500〜800行の追加を見込む。
- **実測値 (PR #6, `master...HEAD`)**: 96 files changed, 2,344 insertions(+), 5,800 deletions(-)（純減 3,456 行）。

---

## 4. 実装順序（1PR内のコミット）

- [x] **Commit 1: REST parity**
  - [x] `GET /api/v1/sessions/:id`、`POST /api/v1/sessions/:id/prompt`、`POST /api/v1/sessions/:id/control`、`POST /api/v1/sessions/import`
  - [x] `GET /api/v1/sessions/:id/events` の `limit` / `before_cursor`
  - [x] ローカル `POST /api/v1/system/shutdown` と `DaemonState::request_shutdown`
  - [x] `SessionDetail` を `client_api` へ移設、リクエスト / レスポンス型を追加
  - [x] cwd → `project_id` は既存の `POST /api/v1/nodes/:id/projects/inspect` で対応可能なことを確認（CLI 結線は Commit 2）
- [x] **Commit 2: CLI の HTTP / WS 移行**
  - [x] `NodeClient` を `reqwest` + `tokio-tungstenite` に書き換え
  - [x] `node.addr` 読み込み、認証付き `/system/info` プローブ、stale addr / 自動起動
  - [x] `fxg node status` を `/system/info` + `/sessions` に移行
  - [x] `fxg node stop` / `node token show` / `node token rotate` を REST / token ファイルに移行
  - [x] `fxg session prompt --wait` の WS 購読（`last_node_seq` で replay をフィルタ）
  - [x] `fxg support` / `fxg doctor` / `fxg logs --session` を既存 REST に移行
  - [x] `fxg session new` / `-w` / `fork` / `import` の cwd・project 解決を REST に移行
  - 注: `fxg internal git-askpass` は Commit 5 で削除済み。
- [x] **Commit 3: Provisioner secret**
  - [x] `ProvisionerDefinition.env_secrets` と `provisioners.env_secrets_json`
  - [x] spawn 時の secret 解決、`secrets::used_by` の Provisioner 走査
  - [x] Settings UI / bindings の更新
- [x] **Commit 4: Provisioner 完全外部化**
  - [x] `compose_bootstrap_script` / `bootstrap.rs` / `bootstrap-workspace` / `project_profiles` の削除
  - [x] `{BOOTSTRAP_SCRIPT}` の置換・検出を削除（互換なし）
  - [x] 外部環境の workspace を `NodeHello` から取得して `StartSession` に使用
  - [x] `provisioner_e2e` を外部コマンド契約に更新
- [x] **Commit 5: askpass / credentials 削除**
  - [x] `git-askpass` サブコマンド、node/hub credentials、GitCredential メッセージ、relay
  - [x] `SessionManager` の `GIT_ASKPASS` 注入削除
  - [x] `git_credentials` テーブル・API・UI・型
- [x] **Commit 6: IPC 削除**
  - [x] `fxg-protocol/src/ipc.rs`、`daemon/ipc.rs`、`ipc_client.rs`、`ipc_framing.rs`
  - [x] `ipc_endpoint` / `FXG_IPC_ENDPOINT` / testutil / logging / Cargo の不要依存
  - [x] `stdio` モードの IPC サーバー起動削除
- [x] **Commit 7: 命名・migration・docs・検証**
  - [x] `PROTOCOL_VERSION = 2`、migration `0015`（状態・イベント名の移行）
  - [x] `Bootstrapping` → `Provisioning`、`BootstrapLog` → `ProvisioningLog`、serde alias
  - [x] docs バナー削除、`AGENTS.md` の同期、取り残し修正
  - [x] `mise run types` / `mise run fmt` / `mise run lint:errors` / `cargo test --workspace`

各コミットは単体で `cargo check --workspace` が通る状態を維持する。機能フラグや旧経路のフォールバックは追加しない。

---

## 5. 受け入れ条件

- [x] IPC 関連コードを削除した状態で、CLI の全コマンドが HTTP / WS 経由で動作する。
- [x] `7860` が使用中でも、既定設定なら ephemeral port にフォールバックし、`node.addr` 経由で CLI / `fxg web` が接続できる。`--listen` 明示時は明確なエラーになる。
- [x] stale な `node.addr` を検出し、healthz 失敗 → 自動起動 → addr 再読込が動作する。
- [x] `prompt` / `control` / `import` / `session show` / `node stop` / `node status` / token 操作の e2e が通る。
- [x] 外部コマンド Provisioner で `NodeHello` → `StartSession` → `DrainAndShutdown` が通り、`env_secrets` が子プロセスに注入される。
- [x] `project_profiles` を持たない DB で起動でき、既存 DB からの migration が通る。
- [x] `PROTOCOL_VERSION = 1` のノードはハブ接続時に明示エラーになる。
- [x] 既存の `bootstrap_log` イベント payload を読み出せる。
- [x] `mise run fmt` / `mise run lint:errors` / `mise run types` / `cargo test --workspace` が通る。

---

## 6. 移行影響・注意

- **既存 Provisioner 定義**: `{BOOTSTRAP_SCRIPT}` を含む定義は動かない。literal が渡るため、docker / VM 側の引数エラーになる。イメージ/起動コマンドへ `fxg` 導入・clone・ツール導入を移す必要がある。
- **Colab / リモートVM**: ゼロタッチではなくなる。起動コマンドに `fxg` の導入と clone を書く参照例を docs に用意する。
- **Git 資格情報**: `git_credentials` のデータは移行されない。アップグレード前に `hub/secrets` へ登録し、Provisioner の `env_secrets` に割り当てる。
- **イベント履歴**: `bootstrap_log` は serde alias で読めるようにする。UI の表示名は `provisioning_log` に統一する。
- **旧ノード / 旧CLI**: 同一バージョンへの更新を前提とする。旧ノードはプロトコル版チェックで拒否し、旧CLIは `fxg node restart` を案内する。
- **ポート**: 既定 7860 が使用中の場合は ephemeral port になるため、外部スクリプトは `node.addr` を読むこと。

---

## 7. 非目標

- `fxg internal stdio --clone` の追加（clone は環境側の責務）。
- ツールチェインの自動検出・自動導入（`mise` / `uv` / `project_profiles`）。
- Local IPC / `git-askpass` / 旧 Provisioner 定義の後方互換。
- ハブを経由しないノード間通信や、新しいネットワークプロトコルの導入。

---

## 8. 実装ログ

- **2026-11-10: Commit 1 (REST parity)**
  - `GET /api/v1/sessions/:id`、`POST /api/v1/sessions/:id/prompt`、`POST /api/v1/sessions/:id/control`、`POST /api/v1/sessions/import` を追加。
  - `GET /api/v1/sessions/:id/events` に `before_cursor` を追加し、`SessionDetail` と各リクエスト / レスポンス型を `client_api` へ移設。
  - `Db::session_event_tail` / `session_events_before` を追加し、`fxg session show` の `event_count` を実際のイベント件数に修正。
  - ローカルノードの `POST /api/v1/system/shutdown` を実装。
  - Node RPC に `ImportSession` を追加し、ハブ経由のインポートにも対応。
  - `fxg-node` の HTTP テストで detail / events / prompt / control / import / shutdown を検証。
- **2026-11-10: Commit 2 (CLI HTTP / WS 移行)**
  - `NodeClient` を `reqwest` ベースに書き換え、`node.addr` と認証付き `/system/info` で接続を検証。
  - `session list/show/prompt/stop/cancel/kill/archive/delete/import/fork/resume`、worktree、token、status、support、logs を REST に移行。
  - `fxg session prompt --wait` は `/api/v1/client/ws` を購読し、`last_node_seq` より後ろのイベントだけを表示。
  - `fxg internal git-askpass` は Commit 5 で削除した。
  - `GET /api/v1/system/info` に `last_synced_at` を追加し、`fxg node status` の表示を維持。
  - 旧デーモンが新しい `node.addr` を削除しないよう、終了時の所有者チェックを追加。
  - `cli_e2e` を HTTP 前提に更新し、`stdio_drain_e2e` と `provisioner_e2e` も通過。
- **2026-11-10: Commit 3 (Provisioner secret)**
  - `ProvisionerDefinition.env_secrets` と migration `0012_provisioner_env_secrets.sql` を追加。
  - spawn 時に `~/.flexagent/secrets/hub/<id>` から値を解決して環境変数へ注入。
  - `secrets::used_by` の Hub 側で Provisioner の参照も検出するよう更新。
- **2026-11-10: Commit 4 (Provisioner 完全外部化)**
  - `bootstrap.rs` / `fxg internal bootstrap-workspace` / `compose_bootstrap_script` / `project_profiles` を削除。
  - migration `0013_remove_project_profiles.sql` を追加。
  - `{BOOTSTRAP_SCRIPT}` の置換を削除し、`{INSTANCE_NAME}` / `{SESSION_ID}` のみ残した。
  - 一時ノードの workspace は `NodeHello.projects` から取得して `StartSession` に渡す。
  - UI の Project profile タブ / ダイアログ / API クライアントを削除。
- **2026-11-10: Commit 5 (askpass / credentials 削除)**
  - `fxg-node/src/credentials.rs` / `fxg-hub/src/credentials.rs` / `fxg internal git-askpass` を削除。
  - `NodeToHubMsg::GitCredentialRequest` / `HubToNodeMsg::GitCredentialResponse` と Hub の relay を削除。
  - `SessionManager` の `GIT_ASKPASS` / `GIT_TERMINAL_PROMPT` 注入を削除。
  - `GitCredentialConfig` / `DEFAULT_GIT_USERNAME` / `hub_settings.git_credentials` と Settings UI の Git credential 編集を削除。
  - migration `0014_remove_git_credentials.sql` を追加し、`git_credentials` テーブルを DROP。
  - `secrets::used_by` は Provisioner の `env_secrets` 参照のみを返すよう変更。
- **2026-11-10: Commit 6 (IPC 削除)**
  - `fxg-protocol/src/ipc.rs`、`fxg-node/src/daemon/ipc.rs`、`ipc_client.rs`、`ipc_framing.rs` を削除。
  - `DaemonConfig.ipc_endpoint`、`paths::ipc_endpoint`、`FXG_IPC_ENDPOINT`、テスト用 IPC エンドポイントを削除。
  - `fxg node start` / `fxg internal stdio` は IPC サーバーを起動しない。
  - `fxg node start` のバナーから IPC Endpoint を削除。
  - `fxg-node` の `libc` 依存、診断の `ComponentName::Ipc` / `IncidentSource::Ipc` を削除。
  - `fxg node status` 用の `LocalStatus` を CLI 側の型として再定義。
  - `daemon/ipc.rs` の Control set_mode 検証は HTTP テストへ移設。
- **2026-11-10: Commit 7 (命名・migration・docs・検証)**
  - `PROTOCOL_VERSION` を `2` に更新。
  - `SessionStatus::Bootstrapping` / `NodeLifecycleStatus::Bootstrapping` を削除し `Provisioning` に集約。
  - `UnifiedEventPayload` / `EventType` / `HubWsMessage` / `ClientEvent` の `BootstrapLog` を `ProvisioningLog` に改名し、`payload_json` 用に `#[serde(alias = "bootstrap_log")]` を追加。
  - migration `0015_rename_provisioning_statuses.sql` で `sessions.status` / `nodes.lifecycle_status` / `session_events.event_type` を移行。
  - UI の `status_bootstrapping` を削除し、`BootstrapLogCard` を `ProvisioningLogCard` に改名。
  - 各 docs の「計画中（未実装）」バナーを削除し、`AGENTS.md` を実装後の構成に同期。
  - D8 の既定ポート使用中時の ephemeral port フォールバックを実装し、明示指定時はエラーにするテストを追加。
  - ブラウザ実機確認で残っていた Git credential 文言と secret-in-use 文言を provisioner 向けに修正。
  - `mise run types` / `mise run sqlx:prepare` / `pnpm --dir ui run check` / `cargo test --workspace` を通過。
