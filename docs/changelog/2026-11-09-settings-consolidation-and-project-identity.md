# 設定のDB集約・プロジェクト同一性・秘密ストア実装計画

- **日付**: 2026-11-09
- **ステータス**: 設計確定 / 未実装
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-node`, `fxg-server`,
  `fxg-cli`, `ui`
- **対象スクリプト / 設定**:
  `mise run fmt` / `mise run lint` / `mise run check` / `mise run types` /
  `mise run sqlx:prepare` / `cargo test --workspace` /
  `pnpm --dir ui run check` / `pnpm --dir ui run test` /
  `~/.flexagent/config.toml`
- **関連ドキュメント**:
  - [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md)
  - [`docs/02-database-schema.md`](../02-database-schema.md)
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md)
  - [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md)
  - [`docs/06-type-safety.md`](../06-type-safety.md)
  - [`docs/changelog/2026-11-08-type-safety-plan.md`](2026-11-08-type-safety-plan.md)
  - [`docs/changelog/2026-11-09-daily-driver-gaps.md`](2026-11-09-daily-driver-gaps.md)

---

## 0. 背景と目的

### 現状の課題

現在、FlexAgent の設定は以下の7箇所に分散している。

1. `~/.flexagent/config.toml`（起動設定・ノード設定・サーバー設定・エージェント設定・Provisioner 設定が混在）
2. リポジトリ内の `.fxg.toml`（プロジェクトキー・表示名・Worktree 設定・一時VM用 Bootstrap 設定）
3. `~/.flexagent/custom_agents.json`（UIから追加したカスタムエージェント）
4. `~/.flexagent/enabled_package_agents.json`（UIから有効化したパッケージエージェント）
5. 環境変数（`FXG_*`）
6. `node.db`（ローカルノードのDB）
7. `server.db`（中央サーバーのDB）

この分散により、以下の問題が起きている。

- **「どれが正（Single Source of Truth）か」が曖昧**:
  `config.toml` と JSON ファイルと DB に同じ種類の設定がまたがり、UI から変更できる項目とファイル編集が必要な項目が分かりにくい。
- **ノードと中央サーバー（ハブ）の役割分担が不明確**:
  本来サーバーが持つべき一時VMの初期化設定（Bootstrap）が各リポジトリの `.fxg.toml` に置かれている。
- **秘密情報の安全な入力経路がない**:
  カスタムエージェントの API キーや Git トークンを UI から安全に登録する仕組みがない。

### 本計画で決定した方針

**「設定ごとに所有者（Node / Server / Client / 起動ファイル）を1つだけに固定し、実行時に変更する設定はすべて所有者の DB に型安全な構造で集約する」**。

具体的な決定事項は以下の9点である。

1. **`.fxg.toml` と旧 JSON 設定ファイルを全面廃止する**:
   リポジトリ内の `.fxg.toml` および `custom_agents.json` / `enabled_package_agents.json` は廃止する。本番未デプロイのためレガシー移行コマンドは作らず、クリーンに DB へ一本化する。
2. **プロジェクト同一性は明示リンク（`project_key`）と Git リモートのみで解決し、表示名は自動導出する**:
   複数マシン間で非 Git プロジェクトを束ねるための識別子（`project_key`）は各ノードの `node.db` に保存する。Worktree からは `git-common-dir` 経由でメイン Worktree のリンクを引き継ぐ。表示名はリポジトリ名・`project_key`・フォルダ名から自動導出する。
3. **Worktree のカスタマイズ設定を全廃する**:
   Worktree の作成先パスは `~/.flexagent/worktrees/{project}/{branch}` に固定し、ベースブランチは現在の `HEAD`（またはコマンド引数）のみとする。作成時のファイルコピー（`copy_files`）や任意コマンド実行（`post_create`）も廃止する。
4. **Provisioner と一時VMの Bootstrap 設定は中央サーバー（`server.db`）で一元管理する**:
   一時VMを起動するのは中央サーバーであるため、Provisioner 定義とプロジェクトごとの初期化スクリプト（`project_profiles`）はすべて `server.db` に置く。
5. **`config.toml` は「プロセス起動時に必要な最小設定」だけに縮小する**:
   待受アドレス、接続先URL、ログ設定、セキュリティ境界（`allowed_hosts` / `allowed_origins`）だけを `config.toml` に残し、Web UI からは読み取り専用（read-only）として表示する。
6. **秘密情報は「ロール別の秘密ファイル（値）＋ DB（メタデータ）」で管理し、UI からは書き込み専用（write-only）にする**:
   トークンや API キーの実体は `~/.flexagent/secrets/{node,server}/<id>`（権限 `0600`）に保存し、DB には名前や種類などのメタデータのみを置く。一度保存した値は API でも UI でも再表示しない。
7. **カスタムエージェントの環境変数を「平文（`env`）」と「秘密参照（`env_secrets`）」に分離する**:
   API キー等は秘密ストアの ID（`SecretId`）を参照する形にし、エージェント起動時に環境変数へ展開する。
8. **バックエンド API は型付き構造体を返し、Web UI で検索可能な設定カタログとして描画する**:
   汎用 Key-Value 型（`SettingKey(String)` / `SettingValue`）は作らず、API・WebSocket プロトコルともに型安全なドメイン構造体（`NodeSettings`, `AgentSettings`, `ServerSettings`, `BootstrapConfigView`）を送受信する。UI 側でそれらを横断検索可能なカタログ画面（出所バッジ付き）として表示する。
9. **ハブ UI からは全設定を編集可能にし、ノード所有の設定は対象ノードへプロキシする**:
   中央サーバーの Web UI でノードの設定を変更すると、ハブは自身の DB にコピーを持たず、対象ノードへ WebSocket 経由で書き込みリクエストを中継（プロキシ）する。ローカルノードの Web UI では、サーバー設定は編集せずハブへのリンクのみを表示する。

### 非目標（やらないこと）

- マルチユーザー／マルチテナント向けの設定分離。
- Git リモートを持たないプロジェクトの自動名寄せ（複数マシンで束ねる場合は、各マシンで同じ `project_key` を手動設定する）。
- DB カラム内での秘密情報の暗号化（秘密の実体は OS のファイル権限で保護されたファイルに置く）。
- `.fxg.toml` や旧 JSON ファイルの後方互換・マイグレーションコマンド（本番未デプロイのため旧コードは直接削除する）。
- プロセス起動前に必要な設定（待受ポート等）の DB 化。

### 受け入れるトレードオフ

- Git リモートがないフォルダを複数マシンで同一プロジェクトとして扱うには、各マシンで同じ `project_key` を設定する必要がある（未設定や typo の場合は別プロジェクト扱いになる）。
- Worktree 作成時の `.env` 自動コピーや `pnpm install` 等の自動実行はなくなる。
- `config.toml` に残る項目（ポート番号や `allowed_hosts` など）は Web UI から変更できず、ファイルの直接編集とプロセス再起動が必要になる。

---

## 1. 設計原則

### 1.1 「その設定を直接使うプロセス」を所有者にする

設定の保存場所は、「どのプロセスがその設定を読んで動作するか」によって一意に決める。

| 設定の分類                               | 設定を使う主体                 | 所有者       | 保存場所                                |
| :--------------------------------------- | :----------------------------- | :----------- | :-------------------------------------- |
| 起動・ネットワーク待受・セキュリティ境界 | プロセス起動処理               | 各マシン     | `config.toml` / 環境変数                |
| ノードの表示名・挙動・スキャン対象       | ノードデーモン                 | ノード       | `node.db`                               |
| エージェント定義・エイリアス・既定値     | ノードデーモン                 | ノード       | `node.db`                               |
| パスと論理プロジェクトの紐づけ           | ノードデーモン                 | ノード       | `node.db`                               |
| Provisioner（一時VM起動コマンド）        | 中央サーバー                   | サーバー     | `server.db`                             |
| 一時VM用のプロジェクト初期化プロファイル | 中央サーバー（VM起動時）       | サーバー     | `server.db`                             |
| トークン・秘密鍵・APIキー等の秘密値      | ノードまたはサーバー           | 各プロセス   | `secrets/{node,server}/` + DBメタデータ |
| 言語・テーマ・ピン留め等の表示設定       | ブラウザ                       | クライアント | `localStorage`                          |
| ノード一覧・プロジェクト一覧・導入Agent  | ハブ（一覧表示・ルーティング） | ノードが報告 | `server.db`（投影キャッシュ）           |

### 1.2 設定データはノード⇔サーバー間で同期・マージしない

- **ノード所有の設定**:
  ハブ（`server.db`）にはコピーを保存しない。ハブ UI でノード設定を閲覧・編集するときは、ハブが対象ノードへリクエストをプロキシする。ノードがオフラインのときは編集不可とする。
- **サーバー所有の設定**:
  ノード（`node.db`）には複製しない。ローカルノード UI からは編集せず、ハブ UI へのリンクのみを表示する。
- **ノードの観測状態（プロジェクトのバインディング一覧・導入済みエージェント一覧）**:
  これらは「設定」ではなく「ノードの現在状態の報告」として扱う。ノードからハブへ **`state_revision`（単調増加する連番）付きの全量スナップショット** を送り、ハブは古いバインディングの削除（prune）を含めて丸ごと置き換える。
- 双方向同期、Last-Write-Wins、競合マージ処理は一切実装しない。

### 1.3 型安全性の徹底（汎用 Key-Value 型の排除）

[`docs/06-type-safety.md`](../06-type-safety.md) の方針に従い、文字列キーや生 JSON を排除する。

- 設定 API や WebSocket メッセージで `SettingKey(String)` + `SettingValue` のような汎用 Key-Value コンテナを使わない。`NodeSettings` / `AgentSettings` / `ServerSettings` / `BootstrapConfigView` といった型付き構造体をそのまま送受信し、`specta` でフロントエンドまで型を一貫させる。
- 設定の出所（`ConfigSource`）、秘密種別（`SecretKind`）、プロジェクト同一性の種別（`ProjectIdentityKind`）はすべて `open_string_enum!` または専用 newtype で定義する。
- `project_id.starts_with("local:")` のようなプレフィックス文字列判定を廃止し、`ProjectIdentityKind` enum で分岐する。

---

## 2. 設定の所有権・保存場所一覧（最終形）

| 設定項目                                                                    | 保存場所                  | Web UI での扱い                       | 変更手段                         |
| :-------------------------------------------------------------------------- | :------------------------ | :------------------------------------ | :------------------------------- |
| `FXG_HOME` / DB ファイルパス                                                | 環境変数 / 既定値         | 読み取り専用                          | 環境変数（再起動が必要）         |
| `node.node_id`                                                              | `config.toml` / env       | 読み取り専用                          | ファイル / env（要再起動）       |
| `node.listen_addr` / `node.central_server_url`                              | `config.toml` / env / CLI | 読み取り専用                          | ファイル / env / CLI（要再起動） |
| `server.listen_addr` / `allowed_hosts` / `allowed_origins`                  | `config.toml` / env / CLI | 読み取り専用                          | ファイル / env / CLI（要再起動） |
| `log.*`（ログレベル・出力先・フォーマット・色）                             | `config.toml` / env / CLI | 読み取り専用                          | ファイル / env / CLI（要再起動） |
| `node.name` / `allow_remote_pty` / `project_scan_dirs` / `tray`             | `node.db`                 | ローカルで直接編集 / ハブからプロキシ | Client API                       |
| `agents.default_agent` / `registry_url` / `registry_cache_ttl_secs`         | `node.db`                 | ローカルで直接編集 / ハブからプロキシ | Client API                       |
| `agents.aliases`                                                            | `node.db`                 | ローカルで直接編集 / ハブからプロキシ | Client API                       |
| `agents.custom`（平文 `env` + 秘密参照 `env_secrets`）                      | `node.db`                 | ローカルで直接編集 / ハブからプロキシ | Client API                       |
| `agents.enabled_package_agents`                                             | `node.db`                 | ローカルで直接編集 / ハブからプロキシ | Client API                       |
| `project_key`（ローカルパス → 論理プロジェクトの明示紐づけ）                | `node.db`                 | ローカルで直接編集 / ハブからプロキシ | Client API / CLI                 |
| `server.vapid_subject`                                                      | `server.db`               | ハブ UI で編集                        | Client API                       |
| `server.git_credentials`（ホスト別の認証方式設定）                          | `server.db`               | ハブ UI で編集                        | Client API                       |
| `provisioners`（一時VMプロビジョナ定義）                                    | `server.db`               | ハブ UI で編集                        | Client API                       |
| `project.bootstrap.setup` / `env`（一時VM用の初期化設定）                   | `server.db`               | ハブ UI（プロジェクト詳細）で編集     | Client API                       |
| `node_token` / `auth_token` / VAPID秘密鍵 / Gitトークン / Agent秘密環境変数 | `secrets/` ファイル       | 書き込み専用（設定済みかのみ表示）    | Secret API                       |
| UIの表示言語（locale）・テーマ・ピン留め状態                                | `localStorage`            | ブラウザ内で即時変更                  | Web UI                           |
| プロジェクト表示名                                                          | 自動導出                  | 表示のみ（編集不可）                  | なし                             |
| ノード一覧・プロジェクトバインディング・導入済みAgent一覧                   | `server.db`（投影）       | ハブ UI で読み取り専用                | ノードからの `NodeHello` 報告    |

---

## 3. 機能別の設計詳細

### 3.1 プロジェクト同一性と `.fxg.toml` の廃止

#### プロジェクトIDの解決順序（`fxg-node/src/project.rs`）

あるディレクトリ `dir` がどの論理プロジェクト（`project_id`）に属するかは、ノード上で以下の優先順位で決定する。

1. **`project_path_links` テーブルの明示リンク（最優先）**:
   - `node.db` の `project_path_links` テーブルから `local_path` の完全一致を探す。
   - 見つからなければ、Git ルートに達するまで親ディレクトリを遡って検索する（モノレポやサブディレクトリ実行に対応するため）。
   - **対象が Git Worktree（`~/.flexagent/worktrees/...` など別ディレクトリにある Worktree）であり、Worktree 側のパスにリンクがない場合は、`git rev-parse --git-common-dir` からメイン Worktree のルートパスを特定し、メイン Worktree 側の `project_path_links` も検索する**。これにより、非 Git リモート環境や手動キー設定環境で Worktree を作成しても別プロジェクトに分裂しない。
   - 一致した場合は、`project_id = "user:<key>"`、`identity.kind = ProjectIdentityKind::UserKey`、`identity.key = Some(key)` とする。
2. **Git リモート URL の正規化**:
   - 明示リンクがなく、`git remote.origin.url` が取得できる場合は、`normalize_git_url` で正規化した URL を `project_id` とする。
   - `identity.kind = ProjectIdentityKind::GitRemote`、`canonical_git_url = Some(remote)`、`identity.key = None` とする。
3. **ローカルパスハッシュへのフォールバック**:
   - 明示リンクも Git リモートもない場合は、そのマシン固有のプロジェクトとして `project_id = "local:<node_id>:<path-hash>"` を割り当てる。
   - `identity.kind = ProjectIdentityKind::Local`、`identity.key = None` とする。

#### プロジェクト表示名（`name`）の自動導出

プロジェクトの表示名は設定項目としては持たず、上記の解決結果から自動導出する。

- `GitRemote` の場合: リポジトリ名（例: `github.com/org/repo` なら `repo`）
- `UserKey` の場合: 設定された `project_key` そのもの
- `Local` の場合: ディレクトリ名

複数ノードから同じ `project_id` に対して異なる `name` が報告された場合、ハブ側は「① メイン Worktree（`is_worktree = false`）からの報告 → ② `last_used_at` が新しいバインディング → ③ `node_id` の辞書順」の優先度で表示名を採用する。

#### `project_key` の設定・変更時の挙動（`fxg project link`）

- CLI の `fxg project link <key>` および API の `POST /api/v1/nodes/:node_id/projects/link` は、`node.db` の `project_path_links` テーブルを更新する。対象ディレクトリが Git Worktree の場合は、メイン Worktree のルートパスに対してリンクを登録する。
- `<key>` にはバリデーション（`[a-zA-Z0-9._-]{1,64}`）をかける。
- すでに登録済みのディレクトリの `project_key` を設定・変更した場合、**既存のセッションも含めて新しい `project_id`（`user:<key>`）へ引き継ぐ**:
  1. 新しい `projects` 行（`user:<key>`）と `project_node_bindings` を upsert する（対象リポジトリの Worktree がある場合はそれらのバインディングも再解決する）。
  2. **`node.db` の `sessions` テーブルのうち、該当パス（および旧バインディング）に属する既存セッションの `project_id` をすべて新しい `project_id` に更新する**。
  3. 同じパスに対する古い `project_id` のバインディングを `delete_other_project_bindings` で削除し、バインディングもセッションもなくなった旧プロジェクトを `delete_orphan_projects` で即座に掃除する。
  4. `nodes.state_revision` を `+1` してハブへ `NodeHello` を再送する。

#### 非Gitプロジェクトの UI 表示

- `ProjectIdentityKind::Local` のプロジェクトには、UI 上で「このマシンだけ（Local only）」バッジを表示する。
- プロジェクト設定ダイアログに `Project key` の入力欄とコピーボタンを配置し、「複数マシンで同じプロジェクトとして共有するには、各マシンで同じ Project key を設定してください」と案内する。

---

### 3.2 ハブへの全量レポートと古いバインディングの削除（Prune）

Worktree 削除時や `project_key` 変更時にハブ（`server.db`）側の `project_node_bindings` や `sessions.project_id` が古いまま残らないよう、ノードからハブへの `NodeHello` を「全量スナップショットによる置換」にする。

1. **ノード側の送信処理**:
   - ノードは自身のプロジェクト一覧・エージェント一覧・ノード設定が変化するたびに `nodes.state_revision` を `+1` し、`NodeHello` に含めて送信する。
   - `NodeHello.sessions`（`SessionSyncState`）にも各セッションの現在の `project_id` を含める。
   - ディレクトリスキャンや DB 読み取りの途中でエラーが起き、プロジェクト一覧を完全には列挙できなかった場合は、**空配列を「全削除」と誤認させないため `projects_complete = false` をセットして送る**。
2. **ハブ側の受信・適用処理（`apply_project_reports`）**:
   - 受信した `state_revision` が、`server.db` に記録されている `nodes.state_revision` より小さい（古い）場合は無視する。
   - `projects_complete = true` の場合は、単一トランザクション内で以下を実行する。
     1. `nodes` テーブルを更新（`state_revision` を進める）。
     2. 報告された `projects` と `project_node_bindings` を upsert する。
     3. **その `node_id` のセッション（`sessions` テーブル）について、バインディングの `local_path` または `SessionSyncState.project_id` に基づき `sessions.project_id` を最新のプロジェクト ID に更新する**（イベント再送 `SessionCreated` 適用時も、現在のバインディングの `project_id` を優先して古い ID に巻き戻らないようにする）。
     4. **その `node_id` に紐づく `project_node_bindings` のうち、今回のレポートに含まれなかった行をすべて削除（prune）する**。
     5. バインディングもセッションも持たなくなった孤立プロジェクトを `delete_orphan_projects` で削除する。
   - `projects_complete = false` の場合は、上記 1〜3（upsert とセッション紐づけ更新）のみを行い、4〜5（削除）はスキップする。
   - ※一時的にディレクトリが見つからない状態（`path_exists = false`）はレポート内に含まれるため削除されない。また、ノードがオフラインになっただけではバインディングは削除しない。

---

### 3.3 秘密情報ストア（Secret Store）

#### ロール別ディレクトリによる保存構造

秘密情報の実体（生文字列）は DB に入れず、`~/.flexagent/` 配下のファイルとして保存する。
**同一マシン上で `fxg daemon` と `fxg server` を同居させた場合でも秘密ファイルが衝突しないよう、`secrets/node/` と `secrets/server/` にディレクトリを分離する**。

```text
~/.flexagent/
├── node_token                  # システム秘密: ノードペアリングトークン（固定パス、0600）
├── auth_token                  # システム秘密: クライアント認証トークン（固定パス、0600）
├── vapid_private.pem           # システム秘密: VAPID 秘密鍵（固定パス、0600）
└── secrets/
    ├── node/
    │   └── <secret-id>         # ノード所有の秘密ファイル（Agent env 等、0600）
    └── server/
        └── <secret-id>         # サーバー所有の秘密ファイル（Git token 等、0600）
```

- 起動時に必要なシステム秘密（`node_token` / `auth_token` / `vapid_private.pem`）は既存のファイルパスを維持しつつ、起動時にそれぞれの DB の `secrets` テーブルへメタデータ行を自動登録して UI の一覧に表示できるようにする。
- ユーザーが追加する秘密（Git トークンやカスタムエージェント用の秘密環境変数）は、`SecretId`（UUID v7）を採番し、ノード所有なら `secrets/node/<id>`、サーバー所有なら `secrets/server/<id>` に保存する。
- ファイル作成時は Unix では `0600`、Windows では実行ユーザーのみアクセス可能な ACL を設定する（既存のトークンファイル書き込み処理と共通化する）。

#### カスタムエージェントの秘密環境変数（`env_secrets`）

`CustomAgentConfig` の環境変数定義を、平文の `env` と秘密参照の `env_secrets` に分ける。

```rust
pub struct CustomAgentConfig {
    pub name: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,          // 平文の環境変数（DBにそのまま保存）
    pub env_secrets: BTreeMap<String, SecretId>, // 環境変数名 → SecretId のマッピング
}
```

- 保存時に `env` と `env_secrets` で同じ環境変数名が重複していた場合はエラーとして拒否する。
- エージェント起動時に `env_secrets` の `SecretId` から `secrets/node/<id>` を読み取り、子プロセスの環境変数に注入する。もし対象の秘密ファイルが存在しない場合は、空文字で起動せず `SECRET_MISSING` エラーで起動を中止する。
- エージェントや Git 認証設定から参照されている秘密を削除しようとした場合は、`SECRET_IN_USE` エラー（参照元のリスト `used_by` 付き）で拒否する。

#### Git 認証設定（`GitCredentialConfig`）の統合

一時VMからの `GitCredentialRequest` に応答するためのサーバー設定に、秘密ストア（`secrets/server/<id>`）を参照する `Secret` プロバイダを追加する。

```rust
pub enum GitCredentialConfig {
    GhCli { username: Option<String> },
    Env { username: Option<String>, token_env: String },
    Secret { username: Option<String>, secret_id: SecretId },
}
```

---

### 3.4 型付き設定 API と Web UI カタログ

#### バックエンドは型付きスナップショットを返す

バックエンドで文字列キーのディスパッチを行わないよう、API はドメインごとの型付き構造体を返す。

- **ローカルノード (`GET /api/v1/settings/node`) / ハブ経由 (`GET /api/v1/nodes/{node_id}/settings`)**:
  `NodeSettingsSnapshot`（起動設定の読み取り専用ビュー `BootstrapConfigView` + `NodeSettings` + `AgentSettings`）を返す。
- **中央サーバー (`GET /api/v1/settings/server`)**:
  `ServerSettingsSnapshot`（サーバー起動設定の読み取り専用ビュー + `ServerSettings` + `Provisioner` 一覧）を返す。
- **更新 API**:
  - `PATCH /api/v1/settings/node`（ハブ経由は `PATCH /api/v1/nodes/{node_id}/settings/node`）: `UpdateNodeSettingsRequest`
  - `PATCH /api/v1/settings/agents`（ハブ経由は `PATCH /api/v1/nodes/{node_id}/settings/agents`）: `UpdateAgentSettingsRequest`
  - `PATCH /api/v1/settings/server`: `UpdateServerSettingsRequest`
  - 各リクエストは `expected_revision: u64` を含み、対象テーブルの `revision` カラムで楽観的ロックを行う。

#### Web UI の構成（`ui/src/routes/settings/+page.svelte`）

フロントエンド側で上記の型付きスナップショットを受け取り、検索・フィルタ可能なカタログ UI として描画する。

- **画面上部**: 全設定項目を横断して絞り込める検索ボックス。
- **タブ構成**: `Bootstrap` / `This machine` / `Projects` / `Server` / `Agents` / `Browser` / `Audit`
- **各項目の表示**:
  - 保存場所を示す出所バッジ（`node.db`, `server.db`, `config.toml`, `env`, `cli`, `secret file`, `localStorage`）を表示する。
  - `Bootstrap`（`config.toml` / `env` / `cli` 由来）の項目は現在の値と出所（CLI フラグ名・環境変数名・設定ファイルパス）を読み取り専用で表示し、編集コントロールを出さない。
  - 秘密情報は `SecretField` コンポーネント（write-only）で扱い、保存後は「設定済み（Configured）」バッジと更新日時・参照元のみを表示する。
- **プロジェクト設定ダイアログ**:
  - `/projects` 画面のカードから設定ダイアログを開き、導出された表示名、同一性バッジ（`Git` / `User key` / `Local only`）、バインディング一覧、`Project key` の編集・コピー欄、および一時VM用 Bootstrap プロファイル編集欄（ハブ UI のみ）を表示する。

---

## 4. データベーススキーマ変更

### 4.1 マイグレーション（`0010_settings_consolidation.sql`）

`crates/fxg-db/migrations/0010_settings_consolidation.sql` を追加する。

- 単一行テーブル（`node_settings`, `agent_settings`, `server_settings`）はすべて `id INTEGER PRIMARY KEY CHECK (id = 1)` に統一する。
- 楽観的ロック用の別テーブルは作らず、各設定テーブル自体に `revision INTEGER NOT NULL DEFAULT 1` カラムを持たせる。更新時は `UPDATE ... SET ..., revision = revision + 1 WHERE id = 1 AND revision = ?` の単一クエリで検証と更新を完結させる。

```sql
-- 1. ノード設定（node.db が使用。単一行テーブル）
CREATE TABLE node_settings (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    name                     TEXT NOT NULL,
    allow_remote_pty         INTEGER NOT NULL DEFAULT 0,
    project_scan_dirs_json   TEXT NOT NULL DEFAULT '[]',
    tray                     INTEGER,
    revision                 INTEGER NOT NULL DEFAULT 1,
    updated_at               INTEGER NOT NULL
);

-- 2. エージェント設定・エイリアス・カスタム定義（node.db が使用）
CREATE TABLE agent_settings (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    default_agent            TEXT,
    registry_url             TEXT,
    registry_cache_ttl_secs  INTEGER,
    revision                 INTEGER NOT NULL DEFAULT 1,
    updated_at               INTEGER NOT NULL
);

CREATE TABLE agent_aliases (
    alias                    TEXT PRIMARY KEY,
    agent_id                 TEXT NOT NULL,
    updated_at               INTEGER NOT NULL
);

CREATE TABLE custom_agents (
    agent_id                 TEXT PRIMARY KEY,
    name                     TEXT,
    command                  TEXT NOT NULL,
    args_json                TEXT NOT NULL DEFAULT '[]',
    env_json                 TEXT NOT NULL DEFAULT '{}',
    env_secrets_json         TEXT NOT NULL DEFAULT '{}',
    updated_at               INTEGER NOT NULL
);

CREATE TABLE enabled_package_agents (
    agent_id                 TEXT PRIMARY KEY,
    updated_at               INTEGER NOT NULL
);

-- 3. ローカルパス → 論理プロジェクトの明示リンク（node.db が使用）
CREATE TABLE project_path_links (
    local_path               TEXT PRIMARY KEY,
    project_key              TEXT NOT NULL,
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL
);

CREATE INDEX idx_project_path_links_key
    ON project_path_links(project_key);

-- 4. サーバー設定・Git認証・Provisioner・一時VMプロファイル（server.db が使用）
CREATE TABLE server_settings (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    vapid_subject            TEXT,
    revision                 INTEGER NOT NULL DEFAULT 1,
    updated_at               INTEGER NOT NULL
);

CREATE TABLE git_credentials (
    host                     TEXT PRIMARY KEY,
    provider                 TEXT NOT NULL,
    username                 TEXT,
    token_env                TEXT,
    secret_id                TEXT,
    updated_at               INTEGER NOT NULL
);

CREATE TABLE provisioners (
    name                     TEXT PRIMARY KEY,
    description              TEXT,
    command                  TEXT NOT NULL,
    args_json                TEXT NOT NULL DEFAULT '[]',
    idle_timeout_secs        INTEGER,
    env_json                 TEXT NOT NULL DEFAULT '{}',
    updated_at               INTEGER NOT NULL
);

CREATE TABLE project_profiles (
    project_id               TEXT PRIMARY KEY,
    bootstrap_setup_json     TEXT NOT NULL DEFAULT '[]',
    bootstrap_env_json       TEXT NOT NULL DEFAULT '{}',
    revision                 INTEGER NOT NULL DEFAULT 1,
    updated_at               INTEGER NOT NULL
);

-- 5. 秘密情報のメタデータ（node.db / server.db 共通。実体の値は secrets/{node,server}/ に保存）
CREATE TABLE secrets (
    id                       TEXT PRIMARY KEY,
    name                     TEXT NOT NULL,
    kind                     TEXT NOT NULL,
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL
);

-- 6. 既存テーブルへのカラム追加
ALTER TABLE projects ADD COLUMN identity_kind TEXT NOT NULL DEFAULT 'local';
ALTER TABLE projects ADD COLUMN project_key TEXT;
ALTER TABLE nodes ADD COLUMN state_revision INTEGER NOT NULL DEFAULT 0;
```

### 4.2 `fxg-db` クレートの変更

- **`crates/fxg-db/src/settings.rs`（新規）**:
  上記テーブルの取得・更新関数を提供する。更新関数は `expected_revision: u64` を受け取り、`WHERE revision = ?` が一致しなければ `DbError::RevisionMismatch` を返す。
- **`crates/fxg-db/src/registration.rs`**:
  - `ProjectRecord` に `identity_kind: ProjectIdentityKind` と `project_key: Option<String>` を追加し、`upsert_project` で保存する。
  - `NodeRecord` に `state_revision: u64` を追加し、`upsert_node` で保存する。
- **`crates/fxg-db/src/queries.rs`**:
  - `list_projects` が返す `ProjectSummary` に `identity: ProjectIdentity` を含める。
- スキーマ変更後、`mise run sqlx:prepare` で `.sqlx` オフラインキャッシュを更新する。

---

## 5. プロトコル・API 型の変更（`fxg-protocol`）

### 5.1 `config.toml` スキーマの縮小（`fxg-protocol/src/config.rs`）

`GlobalConfig` から `[agents]` と `[provisioners]` を外し、起動時に必要な設定のみを残す。また `.fxg.toml` 用の `ProjectConfig` 関連の型はすべて削除する。

```rust
pub struct GlobalConfig {
    pub log: LogConfig,
    pub node: NodeBootstrapConfig,
    pub server: ServerBootstrapConfig,
}

pub struct NodeBootstrapConfig {
    pub node_id: Option<String>,
    pub listen_addr: Option<String>,
    pub central_server_url: Option<String>,
}

pub struct ServerBootstrapConfig {
    pub listen_addr: Option<String>,
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
}
```

- **削除する型**: `ProjectConfig`, `ProjectAgentConfig`, `ProjectWorktreeConfig`, `ProjectBootstrapConfig`, `PROJECT_CONFIG_FILE_NAME`
- **削除する環境変数**: `FXG_NODE_NAME`, `FXG_ALLOW_REMOTE_PTY`（DB設定として UI から変更するため）

### 5.2 型付き設定・秘密・プロジェクト同一性の型定義（`fxg-protocol/src/settings.rs` 新規）

```rust
open_string_enum! {
    pub enum ConfigSource {
        Default => "default",
        ConfigFile => "config_file",
        Env => "env",
        Cli => "cli",
    }
}

open_string_enum! {
    pub enum ProjectIdentityKind {
        GitRemote => "git_remote",
        UserKey => "user_key",
        Local => "local",
    }
}

open_string_enum! {
    pub enum SecretKind {
        NodeToken => "node_token",
        AuthToken => "auth_token",
        VapidPrivate => "vapid_private",
        GitToken => "git_token",
        AgentEnv => "agent_env",
    }
}

pub struct SecretId(String);

/// 読み取り専用の起動設定とその出所
pub struct SourcedValue<T> {
    pub value: T,
    pub source: ConfigSource,
    /// 環境変数名や config.toml のキー名
    pub source_detail: Option<String>,
}

pub struct NodeBootstrapView {
    pub fxg_home: SourcedValue<String>,
    pub node_id: SourcedValue<String>,
    pub listen_addr: SourcedValue<String>,
    pub central_server_url: SourcedValue<Option<String>>,
    pub log_level: SourcedValue<String>,
    pub log_file: SourcedValue<Option<String>>,
    pub log_format: SourcedValue<String>,
    pub log_no_color: SourcedValue<bool>,
}

pub struct ServerBootstrapView {
    pub fxg_home: SourcedValue<String>,
    pub listen_addr: SourcedValue<String>,
    pub allowed_hosts: SourcedValue<Vec<String>>,
    pub allowed_origins: SourcedValue<Vec<String>>,
    pub log_level: SourcedValue<String>,
    pub log_file: SourcedValue<Option<String>>,
    pub log_format: SourcedValue<String>,
    pub log_no_color: SourcedValue<bool>,
}

pub struct NodeSettings {
    pub name: String,
    pub allow_remote_pty: bool,
    pub project_scan_dirs: Vec<String>,
    pub tray: Option<bool>,
    pub revision: u64,
    pub updated_at: i64,
}

pub struct UpdateNodeSettingsRequest {
    pub name: String,
    pub allow_remote_pty: bool,
    pub project_scan_dirs: Vec<String>,
    pub tray: Option<bool>,
    pub expected_revision: u64,
}

pub struct AgentSettings {
    pub default_agent: Option<String>,
    pub registry_url: Option<String>,
    pub registry_cache_ttl_secs: Option<u64>,
    pub aliases: BTreeMap<String, String>,
    pub revision: u64,
    pub updated_at: i64,
}

pub struct UpdateAgentSettingsRequest {
    pub default_agent: Option<String>,
    pub registry_url: Option<String>,
    pub registry_cache_ttl_secs: Option<u64>,
    pub aliases: BTreeMap<String, String>,
    pub expected_revision: u64,
}

pub struct NodeSettingsSnapshot {
    pub bootstrap: NodeBootstrapView,
    pub node: NodeSettings,
    pub agents: AgentSettings,
}

pub struct ServerSettings {
    pub vapid_subject: Option<String>,
    pub git_credentials: BTreeMap<String, GitCredentialConfig>,
    pub revision: u64,
    pub updated_at: i64,
}

pub struct UpdateServerSettingsRequest {
    pub vapid_subject: Option<String>,
    pub git_credentials: BTreeMap<String, GitCredentialConfig>,
    pub expected_revision: u64,
}

pub struct ServerSettingsSnapshot {
    pub bootstrap: ServerBootstrapView,
    pub server: ServerSettings,
}

pub struct SecretSummary {
    pub id: SecretId,
    pub name: String,
    pub kind: SecretKind,
    pub configured: bool,
    pub updated_at: Option<i64>,
    pub used_by: Vec<String>,
}

pub struct UpsertSecretRequest {
    pub name: String,
    pub kind: SecretKind,
    /// 書き込み専用。レスポンスやログには一切含めない。
    pub value: String,
}

pub struct ProjectIdentity {
    pub kind: ProjectIdentityKind,
    pub key: Option<String>,
}

pub struct ProjectProfile {
    pub project_id: ProjectId,
    pub bootstrap_setup: Vec<String>,
    pub bootstrap_env: BTreeMap<String, String>,
    pub revision: u64,
    pub updated_at: i64,
}

pub struct UpdateProjectProfileRequest {
    pub bootstrap_setup: Vec<String>,
    pub bootstrap_env: BTreeMap<String, String>,
    pub expected_revision: u64,
}
```

### 5.3 Node ⇔ Server 間メッセージの追加（`fxg-protocol/src/node_server.rs`）

- `NodeHello` に `state_revision: u64` と `projects_complete: bool` を追加する。
- `SessionSyncState` に `project_id: ProjectId` を追加する（`project_key` 変更後のセッション所属先をハブへ確実に伝えるため）。
- `NodeProjectReport` に `identity: ProjectIdentity` を追加する。
- ハブからノードの設定・秘密を操作するため、`ServerToNodeMsg` に以下を追加する。
  - `GetNodeSettings { request_id: String }`
  - `UpdateNodeSettings { request_id: String, request: UpdateNodeSettingsRequest }`
  - `UpdateAgentSettings { request_id: String, request: UpdateAgentSettingsRequest }`
  - `GetSecrets { request_id: String }`
  - `UpsertSecret { request_id: String, id: SecretId, request: UpsertSecretRequest }`
  - `DeleteSecret { request_id: String, id: SecretId }`
  - `RotateSecret { request_id: String, id: SecretId }`
- `NodeToServerMsg` に `NodeSettingsResult` と `SecretOpResult` を追加する。

### 5.4 追加する REST API エンドポイント

```text
# ローカルノードの設定・秘密 API
GET    /api/v1/settings/node
PATCH  /api/v1/settings/node
PATCH  /api/v1/settings/agents
GET    /api/v1/secrets
PUT    /api/v1/secrets/{secret_id}
DELETE /api/v1/secrets/{secret_id}
POST   /api/v1/secrets/{secret_id}/rotate

# 中央サーバーの設定・秘密・ノードプロキシ API
GET    /api/v1/settings/server
PATCH  /api/v1/settings/server
GET    /api/v1/nodes/{node_id}/settings
PATCH  /api/v1/nodes/{node_id}/settings/node
PATCH  /api/v1/nodes/{node_id}/settings/agents
GET    /api/v1/nodes/{node_id}/secrets
PUT    /api/v1/nodes/{node_id}/secrets/{secret_id}
DELETE /api/v1/nodes/{node_id}/secrets/{secret_id}
POST   /api/v1/nodes/{node_id}/secrets/{secret_id}/rotate
GET    /api/v1/nodes/{node_id}/agents
GET    /api/v1/projects/{project_id}/profile
PUT    /api/v1/projects/{project_id}/profile
```

### 5.5 エラーコード（`ErrorCode`）

以下のエラーコードを追加する（既存で同等のものがある場合は再利用する）。

- `REVISION_MISMATCH`: `expected_revision` の不一致（楽観的ロック競合）
- `SECRET_MISSING`: 参照されている秘密ファイルが存在しない
- `SECRET_IN_USE`: エージェントや Git 認証から参照中の秘密を削除しようとした
- `SECRET_INVALID`: 秘密の ID または値が不正

---

## 6. クレート別の実装変更一覧

### 6.1 `fxg-node`（ローカルノードデーモン）

- **`project.rs`**:
  - `.fxg.toml` の読み書き（`load_project_config` / `patch_project_key`）を削除する。
  - `resolve_project(dir, node_id, db)` で `project_path_links`（メイン Worktree へのフォールバック検索を含む） → Git リモート → `local:` の順に解決し、`ProjectIdentity` を返す。
- **`worktree.rs` / `daemon/ops.rs`**:
  - Worktree 作成先を `~/.flexagent/worktrees/{project}/{branch}` に固定し、`copy_files` / `post_create` の実行処理を削除する。
  - Worktree 削除時および `project_key` 変更時は、バインディングを更新した上で `state_revision` を進めて `NodeHello` を再送する。
- **`daemon/mod.rs` / `daemon/api.rs`**:
  - `AgentCatalogStore`（`Arc<RwLock<AgentSettingsData>>`）を導入し、起動時に `node.db` からエージェント設定・エイリアス・カスタムエージェント・有効化パッケージを読み込む。
  - `custom_agents.json` / `enabled_package_agents.json` のファイル読み書きを廃止し、すべて DB 更新＋`AgentCatalogStore` の再読み込み＋`NodeHello` 再送に統一する。
- **`bootstrap.rs`（一時VM内のワークスペース初期化）**:
  - `.fxg.toml` の読み取りを廃止し、サーバーから渡される環境変数 `FXG_PROJECT_PROFILE_B64`（Base64 エンコードされた `ProjectProfile` JSON）または `--profile` 引数から `setup` コマンドと `env` を受け取って実行する。
  - ツールの自動導入（`mise install`）はリポジトリ内の `mise.toml` / `.tool-versions` / パッケージマニフェストからの自動検出のみとする。
- **`paths.rs` / `credentials.rs`**:
  - `~/.flexagent/secrets/node/` および `~/.flexagent/secrets/server/` ディレクトリの管理と、`GitCredentialConfig::Secret` のファイル読み取りを追加する。

### 6.2 `fxg-server`（中央サーバー）

- **`server.rs` / `provisioner.rs`**:
  - 起動時に `server.db` から `server_settings`（`vapid_subject`）、`provisioners`、`git_credentials` を読み込む。
  - 一時VMセッション起動時（`spawn_session`）に `project_profiles` テーブルから該当プロジェクトのプロファイルを取得し、`FXG_PROJECT_PROFILE_B64` 環境変数として注入する。
- **`hub.rs` / `state.rs` / `api/mod.rs`**:
  - `NodeHello` 受信時の `apply_project_reports` を §3.2 の「全量置換 + prune」に置き換える。
  - ノード設定・ノード秘密の WebSocket プロキシ処理（`GetNodeSettings` / `UpdateNodeSettings` / `UpdateAgentSettings` / `GetSecrets` 等）を実装する。
  - `list_agents` で従来の「オンラインの適当な1ノードを代表として返す」挙動を廃止し、`node_id` を明示指定して取得するようにする。

### 6.3 `fxg-cli`（CLI と `usage-rs` の役割分担）

`usage-rs` は引き続き CLI 引数のパース（`#[derive(Cli, Args, Subcommands)]`）専任とし、`usage-rs` の `config` 機能は導入しない。本番未デプロイのためレガシー移行コマンドや過剰な設定サブコマンドも作らず、既存コマンドをシンプルに新仕様へ合わせる。

- **`fxg daemon` / `fxg server`（起動設定と DB 設定の分離）**:
  - `node.db` に移る `allow_remote_pty` は、CLI フラグ（`DaemonArgs` の `--allow-remote-pty`）からも削除して DB 設定に一本化する（`--tray` / `--no-tray` は起動時の一時オーバーライドとして維持し、未指定時は `node.db` の `node_settings.tray` を参照する）。
  - 起動設定（`--listen`, `--server-url`, `--port`, `--log-*`）の優先順位は `CLI フラグ (usage-rs) > 環境変数 (FXG_*) > config.toml > 既定値` とし、CLI フラグで指定された項目は `SourcedValue` に `source = ConfigSource::Cli`（`source_detail = Some("--port")` 等）を記録して UI の設定画面に出所表示する。
- **`fxg project link <key>`**: `.fxg.toml` ではなく Daemon API 経由で `project_path_links` を更新する。
- **`fxg project info`**: `ProjectIdentityKind` と `project_key` を表示する。
- **`fxg worktree add`**: `--base` 引数のみを受け付け、`.fxg.toml` を参照しない。
- **`fxg bootstrap-workspace`**: `--profile <path>` および `FXG_PROJECT_PROFILE_B64` に対応する。
- **`fxg agents`**: カスタムエージェントの `env_secrets` の参照名を表示する（秘密の実体値は出さない）。

---

## 7. 実装フェーズ

### Phase 0: 型・DBスキーマ・設定スナップショット API の土台

- [ ] `fxg-protocol/src/settings.rs` を新設（`NodeSettingsSnapshot`, `ServerSettingsSnapshot`, `SecretSummary`, `ProjectIdentity`, `ProjectProfile` 等）。
- [ ] `fxg-protocol/src/config.rs` を起動設定（bootstrap）のみに縮小し、`ProjectConfig` 系を削除。
- [ ] `0010_settings_consolidation.sql` マイグレーションを追加。
- [ ] `fxg-db/src/settings.rs` を新設し、`registration.rs` / `queries.rs` を更新。
- [ ] `mise run sqlx:prepare` で `.sqlx` を更新。
- [ ] `GET /api/v1/settings/node` および `GET /api/v1/settings/server` を実装。
- [ ] `mise run types` で TypeScript 型定義（`bindings.ts`）を更新。

### Phase 1: プロジェクト同一性とハブの全量レポート置換

- [ ] `resolve_project` を `project_path_links`（Worktree の `git-common-dir` 解決を含む） → Git リモート → `local:` の解決順に変更。
- [ ] `fxg project link` / REST API / UI を `project_path_links` テーブルへの書き込みに変更。
- [ ] `project_key` 変更時にローカルおよびハブの既存セッション（`sessions.project_id`）を新しい `project_id` へ移行し、旧プロジェクトを `delete_orphan_projects` で削除する処理を実装。
- [ ] `ProjectIdentity` を `ProjectSummary` と `NodeHello` に追加し、表示名の自動導出ルールを実装。
- [ ] ハブの `apply_project_reports` を `state_revision` 検証付きの「全量置換 + セッション再紐づけ + prune」に変更（`projects_complete = false` の安全弁を含む）。

### Phase 2: ロール別秘密情報ストア

- [ ] `paths.rs` に `~/.flexagent/secrets/{node,server}/` 管理（パーミッション `0600` / Windows ACL）を追加。
- [ ] `secrets` テーブルと Secret REST API（一覧・write-only 保存・削除・ローテーション・`SECRET_IN_USE` 検査・監査ログ）を実装。
- [ ] `CustomAgentConfig.env_secrets` の保存とエージェント起動時の環境変数展開を実装。
- [ ] `GitCredentialConfig::Secret` と `git_credentials` テーブルを実装。
- [ ] Web UI に `SecretField` コンポーネントと `AgentManager` の秘密環境変数トグルを追加。

### Phase 3: 設定の DB 集約と `.fxg.toml` / Worktree フックの削除

- [ ] `node_settings` / `agent_settings` / `agent_aliases` / `custom_agents` / `enabled_package_agents` を `node.db` ベースに切り替え。
- [ ] `AgentCatalogStore` を導入し、`custom_agents.json` / `enabled_package_agents.json` の読み書きコードを削除。
- [ ] `server_settings` / `provisioners` / `git_credentials` / `project_profiles` を `server.db` ベースに切り替え。
- [ ] `bootstrap-workspace` を `FXG_PROJECT_PROFILE_B64` / `--profile` 入力に切り替え。
- [ ] Worktree の作成先パスを固定し、`dir_template` / `copy_files` / `post_create` および `.fxg.toml` 関連コードをすべて削除。

### Phase 4: ハブ ⇔ ノード間の設定プロキシと UI 仕上げ

- [ ] WebSocket メッセージ（`GetNodeSettings` / `UpdateNodeSettings` / `UpdateAgentSettings` / `NodeSettingsResult` / `SecretOpResult`）とハブの中継処理を実装。
- [ ] 設定画面の検索可能なカタログ UI・出所バッジ・インライン編集を実装。
- [ ] ハブ UI からのノード設定・秘密編集（オフライン時は読み取り専用）と、ローカルノード UI の `Server` タブのハブリンクを実装。
- [ ] `/projects` 画面のプロジェクト設定ダイアログ（`Project key` 編集・同一性バッジ・Bootstrap プロファイル編集）と `ui/messages/en.json` の翻訳文言を追加。

### Phase 5: 最終検証とドキュメント更新

- [ ] `mise run fmt` / `mise run lint` / `mise run check:ui` / `mise run test` / `mise run types` / `mise run sqlx:check` をすべて通過させる。
- [ ] `AGENTS.md`, `docs/01`, `docs/02`, `docs/03`, `docs/05`, `docs/06`, `docs/troubleshooting.md` を更新。

---

## 8. テスト計画・リスク対策

### 8.1 重点テスト項目

- **Rust 単体・結合テスト**:
  - `resolve_project` の優先順位（明示リンク > Git リモート > `local:`）、サブディレクトリからの親遡り解決、および別ディレクトリの Git Worktree からメイン Worktree の `project_path_links` を解決できること。
  - `project_key` 設定・変更時に既存セッションの `sessions.project_id` が新しい `project_id` に移行され、孤立した旧プロジェクトが削除されること（ローカル・ハブ両方）。
  - ハブの `apply_project_reports` で Worktree 削除後に古いバインディングが消えること、および `projects_complete = false` や古い `state_revision` のレポートでバインディングが誤削除されないこと。
  - `env_secrets` の環境変数展開、秘密ファイル欠損時の `SECRET_MISSING` エラー、参照中秘密の削除ブロック（`SECRET_IN_USE`）、`secrets/node/` と `secrets/server/` の分離。
  - ハブからノードへの設定・秘密プロキシ（オンライン成功・オフライン時 `NODE_OFFLINE`・`expected_revision` 不一致時 `REVISION_MISMATCH`）。
- **UI / E2E テスト**:
  - 設定カタログの出所バッジ表示、`config.toml` 項目の read-only 表示、秘密フィールドの値が再表示されないこと。
  - 2台のノードで非 Git プロジェクトに同じ `project_key` を設定した際、ハブ上で同一プロジェクトに束ねられること。

### 8.2 リスクと対策

| リスク                                                           | 対策                                                                                           |
| :--------------------------------------------------------------- | :--------------------------------------------------------------------------------------------- |
| 非 Git プロジェクトでキー未設定のまま別プロジェクトに分裂する    | UI で「このマシンだけ」バッジと案内を表示し、`Project key` をワンクリックでコピー可能にする。  |
| 別ディレクトリの Worktree で明示 `project_key` が外れる          | `git-common-dir` からメイン Worktree のパスを特定し、`project_path_links` を引き継ぐ。         |
| 同一ホストで `fxg daemon` と `fxg server` の秘密ファイルが衝突   | 保存先を `~/.flexagent/secrets/node/` と `~/.flexagent/secrets/server/` に分離する。           |
| プロジェクト一覧の取得失敗時にハブのバインディングが全消去される | `projects_complete = false` フラグを送り、ハブ側で prune（削除処理）をスキップする。           |
| ネットワーク遅延で古い `NodeHello` が後から届き状態を巻き戻す    | `nodes.state_revision` を比較し、古いリビジョンのレポートを破棄する。                          |
| 秘密値がログやエラーレスポンスに漏洩する                         | API は write-only とし、監査ログにもメタデータのみを記録。ログ出力の redaction を徹底する。    |
| UI からの Provisioner 編集による任意コマンド実行リスク           | 監査ログへの記録、変更時の確認ダイアログ、`allowed_hosts` / トークン認証による保護を徹底する。 |
