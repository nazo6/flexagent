# OpenCode の Tier 2 移行と custom extension 参照（Phase 4）

- **日付**: 2026-11-11
- **状態**: 実装済み（実 opencode v2.2.7 で確認済み）
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-acp`, `fxg-node`, `ui`,
  `docs`
- **対象スクリプト**: `mise run fmt`, `mise run types`,
  `mise run sqlx:prepare`, `mise run lint:errors`,
  `cargo test -p fxg-acp -p fxg-db -p fxg-node -p fxg-cli`,
  `pnpm --dir ui run check`

> 3層モデル全体の設計は
> [`docs/changelog/2026-11-11-agent-extension-architecture.md`](./2026-11-11-agent-extension-architecture.md)
> を参照。本ファイルは Phase 4 の実装記録です。

---

## 1. 目的

最初の Tier 2 実例として `opencode` を ACP + extension に移行し、ユーザー定義の
ACP エージェントからも組み込み extension を再利用できるようにする。

## 2. 変更内容

### 2.1 OpenCode extension

`crates/fxg-acp/src/agent/opencode.rs` を追加した。

- `metadata()`:
  - 表示名 `OpenCode`
  - 説明と `builtin:opencode` アイコン
- `capabilities()`:
  - model config option の各 `provider/model` 値を
    `{ value, name, providerID, id }` のオブジェクトへ変換する
  - `value` は元の ACP ID のまま保持し、`session/set_config_option` には従来どおり
    その ID が送られる
  - `name` は `opencode-go/gpt-6-luna` → `GPT 6 Luna` のように読みやすくする
  - `providerID` は UI の補助ラベル（`opencode-go`）に使われる
- 表示名の変換:
  - 既知のブランド表記（DeepSeek / MiniMax / MiMo / LongCat / GLM / GPT など）
  - `40b` → `40B` のようなサイズ表記
  - `claude-haiku-5-5` → `Claude Haiku 5.5` のような連続する数字の結合

`crates/fxg-acp/src/agent/builtin.rs` の `opencode` provider を
`AgentBackend::AcpExtended(Arc::new(OpenCodeExtension))` に変更した。追加は
extension モジュール1個 + provider の backend 指定のみである。

### 2.2 `CapabilitiesSnapshot`

`AgentExtension::capabilities()` に現在の能力情報を渡す
`CapabilitiesSnapshot` を追加した。

- 既存の `ConfigOptionValue` を変換するには現在値が必要なため
- relay は `CapabilitiesUpdated` ごとに snapshot を作り、overlay を再計算する
  - 古い snapshot の `Replace` が、ユーザーが選択した新しいモデルを巻き戻すのを
    防ぐ

### 2.3 `CustomAgentConfig.extension`

- `fxg-protocol`: `CustomAgentConfig.extension: Option<String>` を追加
- `fxg-db`: migration `0016_custom_agent_extension.sql` で
  `custom_agents.extension` を追加し、SELECT / UPSERT を更新
- `fxg-acp`:
  - `AgentCatalog` が builtin extension を ID で保持する
  - custom 定義に `extension` がある場合、`DriverKind::AcpExtended` と
    `AgentLaunchSpec.extension` を設定する
  - extension の `prepare_launch` / metadata を適用する
  - `driver_for` は extension ID で `ExtendedAcpDriver` をキャッシュする
  - 一覧では custom 定義を `acp_extended` として返し、extension のアイコン・説明を
    補う
  - Tier 3 provider の ID は `is_driver_backed()` で予約し、custom 定義での上書きを
    拒否する
- `fxg-node`:
  - `UpsertCustom` で未知の extension ID と予約 ID を拒否する
- UI:
  - カスタムエージェント編集に extension 選択を追加
  - 保存先の古い `custom_agents.json` / `config.toml` 表記を `node.db` に修正

### 2.4 アイコンと backend バッジ

- `ui/src/lib/components/agent/AgentIcon.svelte` を追加した。
  - `AgentSummary.icon` の `builtin:opencode` を同梱アイコン
    (`ui/static/agents/opencode.png`) に解決する
  - `builtin:antigravity` / `builtin:copilot` /
    `builtin:qwen` / `builtin:cline` / `builtin:auggie` / `builtin:kimi` は
    `agent-glyphs.ts` のインライン SVG で描画し、`currentColor` により
    ライト / ダークテーマへ追従させる
  - `http(s)` / `data:` / ルート相対パスは画像 URL として扱う
  - 未知のキーや未指定は lucide の bot アイコンにフォールバックする
- エージェント一覧 (`AgentManager.svelte`):
  - カードの見出しを `AgentIcon` に置き換えた
  - `AgentSummary.backend` をバッジで表示する
    (`ACP` / `ACP + extension` / `Custom driver`)
- セッション作成画面 (`NewSessionChat.svelte`):
  - Agent ピッカーのトリガーと各候補に `AgentIcon` を表示する
- `ui/src/lib/agents.ts` / `ui/messages/en.json`:
  - backend ラベルのヘルパーと英語文言を追加した

### 2.5 ビルトイン provider の拡充

ネイティブ CLI が `PATH` にある場合に Registry の `npx` より優先される
preset を追加した。

- `opencode-v2`: `opencode acp` を `OpenCodeV2Extension` で拡張する。
  Registry の `opencode-v2` 行と同じ ID に合成される。
- `agy`: Antigravity アイコンを追加した。
- `github-copilot-cli`: `copilot --acp`
- `qwen-code`: `qwen --acp`
- `cline`: `cline --acp`
- `auggie`: `auggie --acp`
- `kimi-code`: `kimi acp`

> Gemini CLI は上流で deprecated のため、後に builtin / alias を削除した。
> 必要な場合は Registry から import して custom 定義として使う。

`AgentCatalog::list` は、preset の実行ファイルが見つかった場合に Registry 行を
preset 行で置き換える。これにより、Registry が `npx` しか知らないエージェント
でも、グローバル導入済みの CLI が一覧・起動の両方で使われる。version と説明は
Registry の値を維持し、アイコンはビルトインのものを使う。

> この Registry 行のマージは、後の
> [`2026-11-11-registry-import.md`](./2026-11-11-registry-import.md) で削除した。
> 現在の一覧は builtin + custom のみで、Registry は import 専用である。

`--acp` / `acp` は各 CLI の `--help` で確認済み（Copilot / Qwen / Cline /
Auggie / Kimi）。

## 3. 実 opencode での確認

opencode v2.2.7（`opencode acp`）で以下を確認した。

1. `GET /api/v1/nodes/:node_id/agents`:
   - `opencode.backend = "acp_extended"`
   - `icon = "builtin:opencode"`
   - 説明が extension metadata に置き換わる
2. `POST .../agents/opencode/probe`:
   - model option が `name` / `providerID` / `id` を持つオブジェクトになる
   - 例: `opencode-go/gpt-6-luna` → `GPT 6 Luna`
3. カスタム定義 `my-opencode`（`extension: "opencode"`）:
   - 一覧で `backend = "acp_extended"`、`sources = ["custom"]`
   - probe が同じ model labels を返す
4. `fxg session new my-opencode`:
   - セッションが起動し、`agent_bound` / `capabilities_updated` / `idle` が記録される
5. `SetConfig` で model を変更:
   - `{ value, name, providerID, id }` の値が受理される
   - 更新後の `capabilities_updated` に新しい `current_value` が反映される
6. 未知の extension ID を保存:
   - `custom agent ... references an unknown extension: missing` で拒否される

## 4. テスト

- `fxg-acp`:
  - OpenCode model label の変換（値の保持、ACP 値への逆変換、非 model 値の保持）
  - 表示名の変換（ブランド、サイズ、バージョン）
  - custom 定義 + builtin extension の `AcpExtended` 解決
  - 未知 extension / 予約 ID の拒否
- `fxg-node`:
  - 既存の session manager / sync E2E が回帰しないこと
- UI:
  - `svelte-check` / `tsc` が通過
  - `AgentIcon` が `builtin:opencode` を解決し、未指定時に bot アイコンへ
    フォールバックすること（ブラウザで確認）

## 5. 変更ファイル

| ファイル                                                   | 変更内容                                                       |
| :--------------------------------------------------------- | :------------------------------------------------------------- |
| `crates/fxg-acp/src/agent/opencode.rs`                     | OpenCode / OpenCode V2 extension                               |
| `crates/fxg-acp/src/agent/builtin.rs`                      | opencode を Tier 2 に登録 / ネイティブ preset                  |
| `crates/fxg-acp/src/agent/extension.rs`                    | `CapabilitiesSnapshot`                                         |
| `crates/fxg-acp/src/agent/extended.rs`                     | overlay の再計算                                               |
| `crates/fxg-acp/src/agent/catalog.rs`                      | extension レジストリ / custom 解決 / preset と Registry の合成 |
| `crates/fxg-acp/src/driver.rs`                             | `AgentLaunchSpec.extension`                                    |
| `crates/fxg-protocol/src/settings.rs`                      | `CustomAgentConfig.extension`                                  |
| `crates/fxg-db/migrations/0016_custom_agent_extension.sql` | `custom_agents.extension`                                      |
| `crates/fxg-db/src/settings.rs`                            | extension の SELECT / UPSERT                                   |
| `crates/fxg-node/src/daemon/ops.rs`                        | extension / 予約 ID の検証                                     |
| `ui/src/lib/components/settings/AgentManager.svelte`       | extension 選択 / `node.db` 表記 / アイコン / backend バッジ    |
| `ui/src/lib/components/session/NewSessionChat.svelte`      | Agent ピッカーのアイコン                                       |
| `ui/src/lib/components/agent/AgentIcon.svelte`             | 組み込みアイコン / URL / フォールバック                        |
| `ui/src/lib/components/agent/agent-glyphs.ts`              | インライン SVG（`currentColor`）                               |
| `ui/src/lib/agents.ts`                                     | backend ラベル                                                 |
| `ui/static/agents/opencode.png`                            | OpenCode アイコン                                              |
| `ui/messages/en.json`                                      | extension / backend の文言                                     |
| `ui/src/lib/generated/bindings.ts`                         | 型の再生成                                                     |
| `.sqlx/`                                                   | `mise run sqlx:prepare` の結果                                 |

## 6. 次のフェーズ

- 完了（Phase 5 のドキュメント更新まで実施済み）
