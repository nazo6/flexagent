# 2026-11-08 エージェント選択 UX と ACP 解決の刷新

- **日付**: 2026-11-08
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-node`, `fxg-server`,
  `fxg-cli`, `ui`
- **対象スクリプト**: `mise run fmt` /
  `cargo test -p fxg-protocol export_bindings` /
  `pnpm --dir ui run check`

## 背景

新規セッションのエージェント選択が `NodeSummary.installed_agents` (ID
文字列のみ) を
素の `<select>` に並べるだけ、設定 > エージェント管理は ACP Registry 全件
(未導入・npx/uvx 含む) をフラットに表示していた。さらに `launch_spec` が binary
配布を起動時に暗黙ダウンロードし、`is_installed` が npx/uvx を常に「導入済み」と
判定していたため、「何が実際に使えるのか」が読み取りづらかった。

## 変更内容

### 1. 起動解決を「ローカル優先・明示インストール」に変更 (`fxg-acp`)

- `AcpRegistry::launch_spec` の解決順を
  **カスタム → fxg 管理バイナリ → PATH 上の既存インストール → npx → uvx**
  に整理。
- **起動時の暗黙ダウンロードを廃止**。未導入なら `fxg agents install <id>`
  を促す
  エラーを返す。
- `AcpRegistry::install` から「PATH
  に同名コマンドがあればそれを返す」フォールバックを
  削除 (外部インストールは起動解決側で扱う)。
- `local_sources()` で利用可能な起動経路を判定し、`AgentListEntry.sources`
  として公開。
  npx/uvx はランナーが `PATH` に存在する場合のみ利用可能と判定する。

### 2. カスタム ACP エージェントの CRUD (`fxg-acp` / `fxg-protocol` / `fxg-node` / `fxg-server`)

- カスタム定義の保存先を追加: `~/.flexagent/custom_agents.json`
  (UI 管理。`config.toml` の `[agents.custom.<id>]`
  とマージし、同名はファイル側が優先)。
- `AcpRegistry::upsert_custom` / `remove_custom` と atomic write を実装。
- `AgentAction` に `UpsertCustom { agent_id, config }` /
  `RemoveCustom { agent_id }` を追加。
- REST: `PUT` / `DELETE /api/v1/nodes/:node_id/agents/custom/:agent_id`
  (body は `CustomAgentConfig`)。ノード/サーバー双方で監査ログ `AGENT_MANAGE`
  を記録。

### 3. `AgentSummary` の再設計 (`fxg-protocol`)

`distributions` / `builtin` / `custom` / `installed` / `installed_versions`
を廃止し、
実態を表すフィールドへ置き換えた。

| フィールド               | 意味                                                        |
| :----------------------- | :---------------------------------------------------------- |
| `sources: AgentSource[]` | `managed` / `external` / `package` / `custom` (空 = 未導入) |
| `registry`               | ACP Registry カタログに存在するか                           |
| `installable`            | 現プラットフォーム向け binary を fxg で導入できるか         |
| `managed_versions`       | fxg が導入済みのバージョン                                  |
| `external_path`          | PATH から検出した実行ファイル                               |
| `custom_config`          | カスタム定義の中身 (UI 編集用)                              |
| `installed_nodes`        | 利用可能なノードID (NodeHello の `installed_agents` 由来)   |

`installed_agents` の意味も「導入済み」から「利用可能」へ変更した。

### 4. UI

- **設定 > エージェント管理** (`AgentManager.svelte`)
  - 既定表示を「このノードで使えるエージェント」だけに変更。
  - 未導入のレジストリ分は **「カタログから追加」ダイアログ** (検索付き)
    へ退避。
  - 起動形態バッジ (fxg 管理 / 既存インストール / npx・uvx / カスタム) と
    外部インストールのパスを表示。
  - **カスタム追加・編集・削除** フォームを追加 (`command` / `args` / `env`)。
- **新規セッション** (`NewSessionChat.svelte`)
  - `GET /api/v1/agents` からノード単位の一覧を引き、表示名 + 起動形態で選択。
  - 「IDを直接入力」トグルで任意 ID 入力へ切替可能
    (カタログ取得失敗時も継続可)。
  - 「エージェントの追加・管理」から `/settings?tab=agents` へ遷移。
- **npx/uvx は既定で無効 (カタログから個別に有効化)**
  - 実体を持たず実行時に取得される `npx` / `uvx` 配布は、既定では一覧に
    表示しない。**「カタログから追加」に並ぶので、インストールと同じ要領で
    エージェントごとに「npx/uvx を有効化」する**。
  - 有効化したエージェントは一覧に `npx/uvx` バッジ付きで表示され、
    「npx/uvx を無効化」で一覧から外せる (実行ファイルは削除しない)。
  - 有効化状態はノードローカル (`~/.flexagent/enabled_package_agents.json`) に
    保存し、REST (`POST` /
    `DELETE /api/v1/nodes/:node_id/agents/:agent_id/package`)
    経由で `AgentAction::SetPackageEnabled` として操作する。
  - `AgentSummary.package_available` は「このノードで npx/uvx を有効化できるか」
    (= レジストリに package 配布があり、ランナーが PATH にある) を表す。
  - 有効化は一覧・選択肢の制御のみで、`launch_spec` の解決順は変えない
    (既存セッションや `fxg run <id>` は引き続き動作する)。
- `agentOptionLabel` / `agentSourceLabel` などの共通ヘルパを
  `ui/src/lib/agents.ts` に集約。

### 5. CLI / ドキュメント

- `fxg agents list` は `SOURCES` 列 (managed / external / npx / uvx / custom)
  を表示。
- `fxg agents update` の対象を「fxg 管理で導入済みのもの」に限定。
- `docs/03`, `docs/04`, `docs/05` を新仕様に追従。

## 影響・移行

- `fxg run <agent>` は未導入の binary
  配布エージェントを勝手にダウンロードしなくなった。
  初回は `fxg agents install <agent>` (または UI からインストール) が必要。
- ACP Registry に未登録のコマンドを `agent_id` に直接指定する使い方は廃止。
  `[agents.custom.<id>]` または UI のカスタム追加で登録する。
- `AgentSummary` のフィールド変更に伴い `ui/src/lib/generated/` を再生成済み
  (`cargo test -p fxg-protocol export_bindings`)。

## 未対応

- エージェント認証 (`fxg agents login`) は引き続き未実装。
- マルチノードでノードごとに `registry_url` / `custom` が異なる場合、カタログは
  応答ノード 1 台分のみ (既知の制約)。
