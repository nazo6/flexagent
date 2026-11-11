# ACP Registry を import 専用にし、builtin + custom に統一

- **日付**: 2026-11-11
- **状態**: 実装済み
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-acp`, `fxg-node`,
  `fxg-hub`, `fxg-cli`, `ui`, `docs`
- **対象スクリプト**: `mise run fmt`, `mise run types`,
  `mise run sqlx:prepare`, `mise run lint:errors`,
  `cargo test --workspace`, `pnpm --dir ui run check`,
  `pnpm --dir ui run lint`, `pnpm --dir ui run test`

---

## 1. 背景

以前は ACP Registry を実行時カタログとして使い、builtin provider とマージしていた。
同じ ID のエージェントが Registry と builtin の両方に存在し、name / description /
version / icon の所有者が曖昧になっていた。`agy` と `antigravity-acp` のように
別 ID のエージェントが並ぶ問題もあった。

Registry の実行時の役割を整理すると、次の3つだけだった。

- 未導入エージェントの一覧表示
- 起動レシピ（PATH / npx / uvx）
- npx/uvx の有効化

managed binary の install/update は UI から使われていない。そこで Registry を
**import 専用のテンプレート集**に降格し、実行時カタログを builtin + custom に
統一した。

## 2. モデル

| 層           | 役割                                                              |
| :----------- | :---------------------------------------------------------------- |
| Builtin      | 実行時カタログの標準。native CLI preset / extension / icon を持つ |
| Custom       | ユーザー定義。Registry から import したものもここに入る           |
| ACP Registry | import ダイアログ用。custom 定義を生成するためだけに使う          |

- 一覧は custom → builtin の順で、Registry 行とのマージは行わない。
- 起動は custom → Tier 3 provider → builtin preset の順。
- Registry は `import_summary` / `import_custom_config` だけを提供する。

## 3. 変更内容

### 3.1 Protocol

- `AgentsResponse` に `registry: Vec<RegistryAgentSummary>` を追加。
- `AgentSummary` から `registry` / `package_available` を削除。
- `AgentSource::Package` を削除（import した custom は `Custom`）。
- `RegistryAgentSummary` / `RegistryDistributionKind` を追加。
- `CustomAgentConfig` に `description` / `icon` / `origin` を追加。
- `CustomAgentOrigin`（`manual` / `registry { id, version }`）を追加。
- `AgentAction::SetPackageEnabled` を削除し、
  `AgentAction::ImportRegistryAgent { agent_id, extension }` を追加。
- `ImportRegistryAgentRequest` を追加。

### 3.2 DB

migration `0017_custom_agent_origin.sql`:

```sql
ALTER TABLE custom_agents ADD COLUMN description TEXT;
ALTER TABLE custom_agents ADD COLUMN icon TEXT;
ALTER TABLE custom_agents ADD COLUMN origin_json TEXT;
DROP TABLE IF EXISTS enabled_package_agents;
```

`custom_agents` の SELECT / UPSERT に新しい列を追加した。

migration `0018_drop_default_agent.sql` は `agent_settings.default_agent` を
削除する。この設定はどこからも適用されず、`DEFAULT_AGENT = "opencode"` という
エージェント固有の既定値だけが残っていた。クライアントは利用可能な
エージェントから選択する。

### 3.3 組み込み alias の削除

`AgentSettings::default_aliases` を削除し、builtin 側にも alias を持たない。

- `claude` → `claude-code-acp` は Registry に存在しない ID だったため削除する
- `codex` → `codex-acp` は Registry import 前提の alias のため削除する
- `antigravity` → `agy` も特別扱いをなくすため削除する
  （`agy` を直接使うか、ユーザーが alias を設定する）

ユーザーが設定する alias (`agent_aliases`) は引き続き利用できる。

### 3.4 `fxg-acp`

- `AgentCatalogState` から `enabled_packages` を削除。
- `AcpRegistry::new` / `apply_catalog` から package enablement を削除。
- `AgentCatalog::list` は custom + builtin のみを返す。
- `AgentCatalog::launch_spec` から Registry フォールバックを削除。
- `AcpRegistry::custom_entries` は origin の version、description、icon を返す。
- `AcpRegistry::import_summary` / `import_custom_config` を追加。
  - 起動コマンドは binary（PATH）→ npx → uvx の順で解決する。
  - binary が見つからない場合は unavailable と理由を返す。
- `registry_entries` / `registry_launch_spec` を削除。

### 3.5 Node / Hub / CLI

- `DaemonState::agents_catalog`:
  - `agents` は builtin + custom のみ。
  - `registry` は import 候補。取得は5秒でタイムアウトし、失敗しても
    `agents` は返す。
- `ImportRegistryAgent`:
  - Registry から custom 定義を生成し、`node.db` に保存する。
  - 既存 custom / Tier 3 予約 ID / 未知 extension は拒否する。
- `installed_agents` は `AgentCatalog::list()` だけを使い、ネットワーク不要。
- Hub に `POST /api/v1/nodes/{node_id}/agents/registry/{agent_id}/import` を追加。
- npx/uvx の package enable/disable エンドポイントを削除。
- CLI の launch spec 解決から Registry index を削除。

### 3.6 UI

- エージェント一覧は builtin + custom のみ。
- 「Import from registry」ダイアログ:
  - Registry の name / description / icon / version / distribution を表示
  - import 可能なら「Import」、import 済みなら「Imported」
  - binary-only で CLI が無い場合は理由を表示
- import した custom は通常の custom として編集・削除できる。
- カスタム定義フォームに description を追加し、import 元の icon / origin を
  編集時も保持する。
- npx/uvx の enable/disable ボタンを削除。

## 4. 挙動の変更

- `GET /api/v1/agents` の `agents` に Registry-only の行は含まれない。
  Registry の一覧は `registry` に入る。
- `fxg session new qwen-code` は builtin の `qwen` が PATH にある場合だけ起動する。
  無い場合は Registry から import して npx の custom を作る。
- Gemini CLI は上流で deprecated のため builtin / alias を持たない。必要な場合は
  Registry の `gemini` を import して custom として使う。
- import した custom はスナップショットである。Registry の更新は自動反映しない。
  更新する場合は custom を削除して再 import する。
- 既存の `enabled_package_agents` は migration で削除する。本番未デプロイのため
  データ移行は行わない。

## 5. テスト

- `fxg-acp`:
  - custom entries の metadata / version
  - npx からの import config（command / args / env / origin / description / icon）
  - import summary の available / distribution
  - 不明な registry binary の unavailable 理由
  - catalog の custom + builtin 一覧
- `fxg-db`: custom agent の新しい列の round-trip
- `fxg-node` / `fxg-cli`: 既存のセッション / sync / CLI E2E が回帰しないこと
- UI: `svelte-check` / `tsc` / `oxlint` / `vitest`

## 6. 変更ファイル

| ファイル                                                | 変更内容                                                                  |
| :------------------------------------------------------ | :------------------------------------------------------------------------ |
| `crates/fxg-protocol/src/client_api.rs`                 | `AgentsResponse.registry` / `RegistryAgentSummary` / `AgentSummary`       |
| `crates/fxg-protocol/src/settings.rs`                   | `CustomAgentConfig` metadata / `CustomAgentOrigin` / `default_agent` 削除 |
| `crates/fxg-protocol/src/common/agent.rs`               | import action / package action 削除                                       |
| `crates/fxg-db/migrations/0017_custom_agent_origin.sql` | metadata 列 / package table 削除                                          |
| `crates/fxg-db/migrations/0018_drop_default_agent.sql`  | 未使用の `default_agent` 削除                                             |
| `crates/fxg-acp/src/agent/builtin.rs`                   | built-in alias                                                            |
| `crates/fxg-db/src/settings.rs`                         | custom agent の SELECT / UPSERT                                           |
| `crates/fxg-db/src/db.rs`                               | package enablement API 削除                                               |
| `crates/fxg-acp/src/registry.rs`                        | import summary / config / catalog state                                   |
| `crates/fxg-acp/src/agent/catalog.rs`                   | builtin + custom の一覧・起動                                             |
| `crates/fxg-node/src/daemon/ops.rs`                     | agents catalog / import action                                            |
| `crates/fxg-node/src/daemon/mod.rs`                     | catalog 初期化 / installed agents                                         |
| `crates/fxg-node/src/session_manager/lifecycle.rs`      | launch spec 簡素化                                                        |
| `crates/fxg-hub/src/api/mod.rs`                         | import route                                                              |
| `crates/fxg-hub/src/api/handlers/agents.rs`             | import handler                                                            |
| `crates/fxg-cli/src/commands/agents.rs`                 | catalog 初期化 / launch spec                                              |
| `ui/src/lib/components/settings/AgentManager.svelte`    | import dialog / custom metadata                                           |
| `ui/src/lib/components/agent/AgentIcon.svelte`          | registry icon URL 対応（既存）                                            |
| `ui/src/lib/agents.ts`                                  | source / distribution ラベル                                              |
| `ui/messages/en.json`                                   | import の文言                                                             |
| `ui/src/lib/generated/bindings.ts`                      | 型の再生成                                                                |
| `.sqlx/`                                                | `mise run sqlx:prepare` の結果                                            |
