# データディレクトリ構成の刷新（OS 標準ルート + node/hub 分離）

- **日付**: 2026-11-13
- **状態**: 実装済み（2026-11-13）
- **対象パッケージ（予定）**: `fxg-protocol`, `fxg-db`, `fxg-node`, `fxg-hub`,
  `fxg-acp`, `fxg-cli`, `ui`
- **対象スクリプト / 設定（予定）**: `mise run fmt`, `mise run types`,
  `mise run lint:errors`, `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
  `pnpm --dir ui run check`, `docker compose build`
- **関連ドキュメント**:
  - [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md) §2（実装時に更新）
  - [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md) §7（トークン）
  - [`docs/02-database-schema.md`](../02-database-schema.md)（DB の保存先）
  - [`docs/troubleshooting.md`](../troubleshooting.md)（パス案内）

> 初回デプロイ前のため、**旧レイアウトからの移行機能・互換フォールバックは作らない**。
> 新しい構成を前提に置き換え、既存のローカル開発データは必要に応じて手動で
> 退避・再作成する。この文書は実装前の設計と作業計画を残すためのもの。

## 1. 背景

現在は `fxg_protocol::config::fxg_home()` が返す単一ディレクトリ
（既定 `~/.flexagent`、`FXG_HOME` で上書き）に、性質の異なるファイルが同居している。

- **ロールの混在**: ローカルノードとハブが同じ `auth_token` を読み書きする
  （`crates/fxg-node/src/daemon/mod.rs` と `crates/fxg-hub/src/state/mod.rs`）。
  同一ホストで両方を起動すると、ハブ用に発行した Web アクセストークンが
  そのままノードの API にも通る。`config.toml` も `[node]` / `[hub]` の
  1 ファイルで、どちらの設定かはセクション名でしか区別できない。
- **ライフサイクルの混在**: 手で編集する設定、壊してはいけない DB・秘密、
  消えてよいキャッシュ、再生成される `*.addr`、巨大な `worktrees/`、ログが
  同じ階層にある。バックアップ・権限・削除の単位を揃えられない。
- **OS 標準からの逸脱**: Linux の XDG、macOS の `~/Library`、Windows の
  `%APPDATA%` / `%LOCALAPPDATA%` のどれにも従っていない。
- **パス構築の散在**: `join("logs")` / `join("auth_token")` のような組み立てが
  各クレートに散らばり、`fxg-db` の `role: &str` は型安全性の方針
  （[`docs/06-type-safety.md`](../06-type-safety.md)）に反している。
- **診断情報の乖離**: `fxg doctor` のファイル一覧は手書きで、実際に生成される
  `node.start.lock` が載っておらず、未使用の `agents/` が載っている。

## 2. 目標

次の 3 軸で分離する。

1. **OS 標準のルート**: 設定・データ・実行時状態・キャッシュ・ログを、
   各 OS の慣例に従った別ディレクトリに置く。
2. **ロール**: `node` と `hub` をディレクトリで分け、プロセスは自分の
   ロールのパスだけを解決する。
3. **ライフサイクル**: 手で編集する設定 / 永続データ / 実行時状態 / キャッシュ /
   ログを分け、バックアップ・権限・削除の単位を揃える。

## 3. 目標レイアウト

以下は Linux の既定。macOS / Windows は §3.2 のルートに同じ構造を置く。

```text
<config>/
├── node.toml                       # ノードの起動設定のみ
└── hub.toml                        # ハブの起動設定のみ

<data>/                   # 0700
├── node/
│   ├── node.db  (+ -wal / -shm)
│   ├── auth_token                  # 0600  ノード専用の Web アクセストークン
│   ├── node_token                  # 0600  ハブ接続用のペアリングトークン
│   ├── secrets/<id>                # 0600  ノード所有の秘密値
│   └── worktrees/<project>/<branch>/
└── hub/
    ├── hub.db
    ├── auth_token                  # 0600  ハブ専用の Web アクセストークン
    ├── vapid.json                  # 0600
    └── secrets/<id>                # 0600  ハブ所有の秘密値

<state>/
├── node/
│   ├── node.addr                   # 起動中ノードの待受アドレス
│   └── start.lock                  # CLI の自動起動ロック
└── hub/
    └── hub.addr

<logs>/
├── node/node.log
└── hub/hub.log

<cache>/node/registry.json
```

- Linux では XDG にログ用のクラスがないため、`<logs>` は `<state>` と同じ
  ディレクトリになる（`~/.local/state/flexagent/<role>/<role>.log`）。
- macOS / Windows のログは §3.2 のとおり別ルートに置く。

### 3.1 ライフサイクルごとの責務

| クラス     | 内容                                               | 消えてよいか       | バックアップ         |
| :--------- | :------------------------------------------------- | :----------------- | :------------------- |
| 設定       | `node.toml` / `hub.toml`（待受・許可リスト・ログ） | いいえ（手で編集） | する                 |
| データ     | DB、トークン、秘密値、worktrees                    | いいえ             | する（秘密は別扱い） |
| 実行時     | `*.addr`、起動ロック                               | はい（再生成）     | しない               |
| ログ       | `node.log` / `hub.log`                             | はい               | 必要なら             |
| キャッシュ | ACP Registry                                       | はい               | しない               |

### 3.2 OS ごとのルート

| クラス     | Linux (XDG)                  | macOS                                     | Windows                          |
| :--------- | :--------------------------- | :---------------------------------------- | :------------------------------- |
| 設定       | `$XDG_CONFIG_HOME/flexagent` | `~/Library/Application Support/flexagent` | `%APPDATA%\flexagent`            |
| データ     | `$XDG_DATA_HOME/flexagent`   | `~/Library/Application Support/flexagent` | `%LOCALAPPDATA%\flexagent`       |
| 実行時     | `$XDG_STATE_HOME/flexagent`  | `~/Library/Application Support/flexagent` | `%LOCALAPPDATA%\flexagent`       |
| ログ       | `$XDG_STATE_HOME/flexagent`  | `~/Library/Logs/flexagent`                | `%LOCALAPPDATA%\flexagent\logs`  |
| キャッシュ | `$XDG_CACHE_HOME/flexagent`  | `~/Library/Caches/flexagent`              | `%LOCALAPPDATA%\flexagent\cache` |

- 既存依存の `dirs` 7 で解決できるため、新しいクレートは追加しない。
- `dirs::state_dir()` は macOS / Windows で `None` を返すため、その場合は
  `data_local_dir` にフォールバックする。
- macOS のログだけは `~/Library/Logs` を使う（`dirs` では表現できないため
  `home_dir` から組み立てる）。
- OS のルートが解決できない場合（極端に制限された環境）は、従来と同様に
  カレントディレクトリ配下の `flexagent` にフォールバックする。

### 3.3 `FXG_HOME`（portable モード）

`FXG_HOME` は「製品ルートの上書き」と再定義する。OS ルートの代わりに
`$FXG_HOME/{config,data,state,logs,cache}` をクラスごとのルートとして使い、
ロールのサブディレクトリはそのまま適用される。テストの一時ディレクトリと
Docker のボリュームはこの形になる。

```text
$FXG_HOME/
├── config/
│   ├── node.toml
│   └── hub.toml
├── data/
│   ├── node/
│   │   ├── node.db
│   │   ├── auth_token
│   │   ├── node_token
│   │   ├── secrets/
│   │   └── worktrees/
│   └── hub/
│       ├── hub.db
│       ├── auth_token
│       ├── vapid.json
│       └── secrets/
├── state/
│   ├── node/
│   │   ├── node.addr
│   │   └── start.lock
│   └── hub/
│       └── hub.addr
├── logs/
│   ├── node/node.log
│   └── hub/hub.log
└── cache/
    └── node/registry.json
```

- クラスの対応は OS モードと同じ（`config` / `data` / `state` / `logs` /
  `cache`）。`FXG_HOME` 配下でもクラスの境界が保たれるため、バックアップや
  ボリューム設計を OS モードと共通にできる。
- Docker で `FXG_HOME=/data` のままにすると `data/data/...` になるため、
  コンテナの `FXG_HOME` は `/var/lib/flexagent` などに変える
  （`VOLUME` と `compose.yaml` も合わせて更新する）。
- ログは `<logs>/<role>/<role>.log` に置く（portable では
  `$FXG_HOME/logs/node/node.log`）。

## 4. 設計方針

### 4.1 単一のパス解決 API

`fxg-protocol` に `paths` モジュールを追加し、次の型だけがパスを組み立てる。

- `Role { Node, Hub }`: プロセスのロール。`fxg-db::DbRole` を統合して
  1 つの型にし、`role: &str` を全廃する。
- `Roots { config, data, state, cache, logs }`: OS ルート、または portable モードの
  `$FXG_HOME/{config,data,state,logs,cache}`。
- `Paths { role, roots }`: ロール込みの具体的なパス。
  `config_path` / `db_path` / `auth_token_path` / `secrets_dir` /
  `worktrees_dir` / `state_dir` / `addr_path` / `log_path` / `cache_dir` などを提供する。

不変条件:

- ノードは `Role::Node` の `Paths` だけを持ち、ハブの DB・トークンを参照できない。
- ハブは `Role::Hub` の `Paths` だけを持つ。
- CLI はコマンドの対象（`node` / `hub`）に応じて `Paths` を作り分ける。
- 各クレートは `join(...)` でパスを組み立てない。`Paths` のメソッドだけを使う。

### 4.2 設定ファイルの分割

- `GlobalConfig` を `NodeConfig` / `HubConfig` に分割する。
  `[log]` はそれぞれのファイルが持つ。
- 読み込みは対象ロールのファイルだけ。`FXG_LOG_*` はプロセスごとに
  自分の設定へ適用する。
- `[log] file` の相対パスはロールのログディレクトリ
  (`<logs>/<role>/`) を基準に解決する。
- 未知キーを拒否する現在の方針（`deny_unknown_fields`）は維持する。

### 4.3 トークンの分離

`auth_token` をロールごとに持つ。これにより、同一ホストで node と hub を
動かしても、片方の Web アクセストークンでもう片方へアクセスできなくなる。
`node_token` はノード専用、`vapid.json` はハブ専用として配置する。

### 4.4 権限

- データディレクトリと実行時ディレクトリは Unix で `0700`。
- トークン・秘密値・VAPID 鍵は `0600`。
- 既存の「Windows はユーザープロファイルの ACL に従う」方針は据え置き、
  追加の ACL 操作はこの変更に含めない。

### 4.5 パスを一元化する対象

- `fxg-node/src/paths.rs` の `NodePaths` は `Paths` に置き換える。
- `fxg-db` の `node_db_path` / `hub_db_path` と `secrets::*` の
  `fxg_home` + `role: &str` は `Paths` を受け取る形にする。
- `fxg-acp` の Registry キャッシュと `env_secrets` 解決も `Paths` を使う。
- `fxg-cli` のログ解決、`node.addr` / `hub.addr` 読み取り、`doctor`、
  `logs`、`support`、`tray` も `Paths` に寄せる。
- `fxg-hub` の `HubOptions` / `ProvisionerManager` / Web Push / diagnostics も同様。
- 未使用の `agents/` は削除する。

### 4.6 旧レイアウトの扱い

互換フォールバックも警告も実装しない。既定ルートにデータがなく、旧
`~/.flexagent` だけが存在する場合でも、何も検出せず新しいルートから開始する。
既存の開発データが必要な場合は、利用者が手動で新ルートへコピーする。

## 5. 破壊的変更

| 変更                 | 内容                                                                                                                                                       |
| :------------------- | :--------------------------------------------------------------------------------------------------------------------------------------------------------- |
| データルート         | `~/.flexagent` を廃止し、OS 標準ルート + ロール別サブディレクトリへ                                                                                        |
| 設定ファイル         | `config.toml` → `node.toml` / `hub.toml`                                                                                                                   |
| `FXG_HOME`           | 「ロールのホーム」から「OS ルートの代わりに `{config,data,state,logs,cache}` を持つ製品ルート」へ                                                          |
| Web アクセストークン | node と hub で別ファイル・別トークン                                                                                                                       |
| 実行時ファイル       | `*.addr` / 起動ロックをデータディレクトリから実行時ディレクトリへ                                                                                          |
| API / UI             | `fxg_home` フィールドを `data_dir` に改名（`NodeBootstrapView` / `HubBootstrapView` / `SupportBundleConfig`）                                              |
| Docker               | `FXG_HOME` をクラス別サブディレクトリ構成に変更（例: `/var/lib/flexagent/{data,state,logs}`）。`VOLUME` と `compose.yaml` の `node_token` マウント例も更新 |
| CLI 表示             | `fxg doctor` / 起動バナー / `fxg logs` のパス表示                                                                                                          |

## 6. 作業計画

各フェーズは独立したコミットにし、フェーズごとにビルド・テストを通す。

### フェーズ 1: パス基盤

- `fxg-protocol` に `paths`（`Role` / `Roots` / `Paths`）を追加し、単体テストを書く。
- `FXG_HOME` の portable モードと OS ルートの解決を実装する。
- 既存の `fxg_home()` と `NodePaths` はこのフェーズではまだ残し、
  呼び出し側の移行はフェーズ 2 以降で行う。

### フェーズ 2: ロール分離と設定分割

- `DbRole` を `Role` に統合し、`fxg-db` のパス関数と `secrets` を `Paths` 化する。
- `NodePaths` を削除し、ノードの daemon / DB / worktree / ACP Registry を
  `Paths` に移行する。
- `HubOptions` とハブの DB / push / provisioner / diagnostics を `Paths` に移行する。
- `GlobalConfig` を `NodeConfig` / `HubConfig` に分割し、読み込み先を
  `node.toml` / `hub.toml` にする。
- `auth_token` をロール別ファイルに切り替える。

### フェーズ 3: CLI・診断・ログ

- `main.rs` のログ初期化、`logging.rs`、`spawn.rs`、`client.rs` を `Paths` 化する。
- `fxg node` / `fxg hub` / `fxg logs` / `fxg doctor` / `fxg support` /
  `fxg web` / `tray` のパス参照を更新する。
- `fxg doctor` のファイル一覧を `Paths` から生成する。
- `node.start.lock` を実行時ディレクトリへ移す。

### フェーズ 4: コンテナ・UI・ドキュメント

- `Dockerfile` / `compose.yaml` のコメントと `node_token` のマウント例を更新する。
- `specta` の型を再生成し、`SettingsCatalog.svelte` と `ui/messages/*.json` の
  `fxg_home` 参照を `data_dir` に更新する。
- `docs/01`, `docs/02`, `docs/03`, `docs/04`, `docs/05`,
  `docs/troubleshooting.md`, リポジトリの `AGENTS.md` のパス表記を更新する。

## 7. 検証

- Rust: `cargo test --workspace` / `cargo clippy --workspace --all-targets` /
  `cargo fmt --all --check`
- 型・UI: `mise run types`（生成物の差分確認）、`pnpm --dir ui run check`、
  `mise run lint:errors`
- フォーマット: `mise run fmt`
- 手動（Linux）:
  - `FXG_HOME` 未設定で `fxg node start` → `~/.local/share/flexagent/node/` に
    DB・秘密・worktree、`~/.local/state/flexagent/node/` に `.addr` とログが
    できること。
  - `fxg hub start` → 別ディレクトリに `hub.db` / `auth_token` / `vapid.json`
    ができ、ノードのトークンと異なること。
  - `fxg doctor` の一覧が実ファイルと一致すること。
  - `fxg logs node` / `fxg logs hub` がロール別のログを表示すること。
  - worktree の作成・削除が新しいルートで動くこと。
- 手動（Docker）: `docker compose build` 後、`$FXG_HOME/data/hub/hub.db` が
  作られ、`$FXG_HOME` 直下に旧ファイルが作られないこと。Web UI と
  ペアリングが動くこと。
- クロスプラットフォーム: `Paths` の単体テストを OS ごとに実行し、
  macOS の `~/Library/Logs` と Windows の `%APPDATA%` / `%LOCALAPPDATA%` の
  解決を確認する。

## 8. リスクと対策

- **worktree の絶対パス**: worktree の移動は git のメタデータと DB の
  `local_path` の両方に影響する。移行機能を作らないため、既存の worktree は
  新ルートでは使えない。開発機では必要に応じて再作成する。
- **`FXG_HOME` の意味変更**: テストと Docker の前提が変わる。テストは
  `<home>/data/node` / `<home>/config/node.toml` を見る形になり、Docker は
  `FXG_HOME` 配下にクラス別ディレクトリが増える。フェーズ 1 で `Paths` の
  テストを portable モード中心に整備し、フェーズ 2 で一括更新する。
- **設定の読み込み漏れ**: CLI が両ロールの設定を読む箇所（`tray` など）は、
  対象ロールを明示して読み込むようにする。`GlobalConfig` の一括読み込みは
  残さない。
- **Windows の権限**: `0700` / `0600` は Unix のみ。Windows は既存方針
  （ユーザープロファイルの ACL）を維持し、この変更では扱わない。

## 9. スコープ外

- 旧レイアウトからのデータ移行コマンド、シンボリックリンク、互換フォールバック
- worktree ルートの設定可能化（必要になったら別途）
- Windows の追加 ACL 操作
- ハブとノードの設定同期・マージ（現行方針どおり行わない）

## 10. 実装メモ（2026-11-13）

計画どおり、同一 PR 内で実装した。実装上の確定事項は次のとおり。

- `fxg-protocol::paths` に `Role` / `Roots` / `Paths` を追加し、`fxg-db::DbRole` を
  `Role` に統合した。`secrets` の `role: &str` も廃止し、`Paths` を受け取る。
- 設定ファイルは `node.toml` / `hub.toml` に分割し、`NodeConfig` / `HubConfig` が
  自分のファイルだけを読む。`[log] file` の相対パスはロールのログディレクトリ
  （`<logs>/<role>/`）基準。
- `NodeBootstrapView` / `HubBootstrapView` / `SupportBundleConfig` の `fxg_home`
  フィールドを `data_dir` に改名した。`ClientApiBackend` には `data_dir()` /
  `db_path()` / `log_path()` を追加し、診断が DB・ログの実パスを返す。
- Worktree の作成先は `{worktrees_dir}/{project}/{branch}`（`Paths::worktrees_dir()`）
  に固定。プレースホルダは `{fxg_home}` から `{worktrees_dir}` に変更した。
- `node.start.lock` は実行時ディレクトリ（`<state>/node/start.lock`）へ移した。
- 未使用だった `agents/` のパスヘルパーを削除した。
- Docker は `FXG_HOME=/var/lib/flexagent` に変更し、`config` / `data` / `state` /
  `logs` / `cache` のサブディレクトリを永続化する。
- 初回デプロイ前の方針どおり、旧レイアウトの移行・警告は実装していない。
- 検証は `cargo test --workspace` / `cargo clippy --workspace --all-targets` /
  `cargo fmt --all --check` / `mise run types` / `pnpm --dir ui run check`。
