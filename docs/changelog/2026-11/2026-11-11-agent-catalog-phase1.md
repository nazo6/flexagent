# AgentCatalog 導入（3層モデル Phase 1）

- **日付**: 2026-11-11
- **状態**: 実装済み（Phase 1 / 挙動変更なし）
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-node`, `fxg-cli`, `ui`
- **対象スクリプト**: `mise run fmt`, `mise run types`,
  `cargo test -p fxg-acp -p fxg-node -p fxg-cli`,
  `cargo clippy -p fxg-acp -p fxg-node -p fxg-cli -p fxg-protocol --all-targets -- -D warnings`,
  `pnpm --dir ui run check`

> 3層モデル全体の設計は
> [`docs/changelog/2026-11-11-agent-extension-architecture.md`](../../proposals/2026-11-11-agent-extension-architecture.md)
> （[PR #8](https://github.com/nazo6/flexagent/pull/8)）を参照。本ファイルは Phase 1
> の実装記録です。

---

## 1. 目的

Phase 2 以降で `AgentExtension` と full custom driver を追加するための土台として、
エージェントの「定義・起動解決・driver 選択」を `AgentCatalog` に集約する。
Phase 1 では **挙動を変えず**、既存の ACP only 経路をそのまま catalog 経由に
置き換える。

## 2. 変更内容

### 2.1 `fxg-acp`: `agent` モジュールの追加

- `AgentDefinition`: 表示名・説明・アイコンなどの静的メタデータ
- `AgentProvider` / `StaticProvider`: ビルトインエージェントの登録口
- `LaunchPreset`: `PATH` 上の CLI を ACP として起動するための preset
- `AgentBackend`: Phase 1 は `Acp` のみ。Phase 2 以降に `AcpExtended` /
  `Custom` を追加する
- `AgentCatalog`:
  - `list()`: custom → builtin preset → ACP Registry の順に一覧を構築する
  - `launch_spec()`: custom → builtin preset → ACP Registry の順に起動定義を解決する
  - `driver_for()`: `DriverKind` から共有 `AcpDriver` を返す
- `builtin::providers()`: 旧 `AGENT_PRESETS`（`opencode` / `agy`）を provider 化

`AcpRegistry` から preset 解決を外し、registry と custom 定義の管理に集中させた。
`launch_spec` は次の2つに分割した。

- `custom_launch_spec()`: custom 定義 + `env_secrets` の解決
- `registry_launch_spec()`: ACP Registry のローカル優先解決

### 2.2 `fxg-node`: `SessionManager` の依存先変更

- `Inner` の `AcpRegistry` を `Arc<AgentCatalog>` に置き換え
- `default_driver_factory()` を削除し、`AgentCatalog::driver_for` を既定の
  factory にした
- `SessionManager::catalog()` を追加（旧 `registry()` を置き換え）
- `with_factory()` はテスト用の driver 注入として維持
- `DaemonState` は catalog を構築して `SessionManager` に渡す。
  `with_driver_factory()` のテスト経路も維持
- `agents_catalog()` / `installed_agents()` / `refresh_agent_catalog()` を
  catalog 経由に更新

### 2.3 `fxg-protocol` / UI

- `AgentBackendKind`（`acp` / `acp_extended` / `custom`）を追加
- `AgentSummary` に `backend` / `icon` を追加
- `AgentSource::Custom` の doc を `node.db` 保存に合わせて修正
- `mise run types` で `ui/src/lib/generated/bindings.ts` を同期

Phase 1 では UI の表示変更は行わない（backend バッジは Phase 2 以降で追加）。

### 2.4 `fxg-cli`

- `fxg agents login/logout` も `AgentCatalog` 経由で起動定義を解決する
- これにより、今後の `prepare_launch` が CLI の認証経路にも適用される

## 3. 挙動の維持

- 解決順位は従来どおり **custom → preset → ACP Registry**
- custom が同名の builtin / registry を上書きする
- `AcpDriver` は catalog に1つだけ保持し、warm pool を共有する
- capability probe / resume / fork / permission の経路は変更しない

## 4. テスト

- `fxg-acp`:
  - custom の優先、preset の追加引数、custom による preset の上書き
  - `DriverKind::Acp` が共有 driver を返すこと
  - preset 定義の一意性
- `fxg-node`:
  - 既存の session manager テスト（86件）と `sync_e2e`（4件）が通過
- UI:
  - `svelte-check` / `tsc` が通過

## 5. 変更ファイル

| ファイル                                                     | 変更内容                                                    |
| :----------------------------------------------------------- | :---------------------------------------------------------- |
| `crates/fxg-acp/src/agent/{mod,provider,builtin,catalog}.rs` | `AgentCatalog` / `AgentProvider` / builtin providers を追加 |
| `crates/fxg-acp/src/registry.rs`                             | preset 解決の除去、custom / registry の一覧・起動解決を分割 |
| `crates/fxg-acp/src/lib.rs`                                  | `agent` モジュールと `AgentCatalog` の公開                  |
| `crates/fxg-node/src/session_manager/mod.rs`                 | `AgentCatalog` への依存、catalog factory                    |
| `crates/fxg-node/src/session_manager/lifecycle.rs`           | 起動定義の解決を catalog 経由に変更                         |
| `crates/fxg-node/src/daemon/{mod,ops}.rs`                    | catalog の構築・一覧・管理操作                              |
| `crates/fxg-node/src/session_manager/tests.rs`               | テスト用 catalog の構築                                     |
| `crates/fxg-cli/src/commands/agents.rs`                      | login/logout を catalog 経由に変更                          |
| `crates/fxg-protocol/src/client_api.rs`                      | `AgentBackendKind` / `AgentSummary.backend` / `icon`        |
| `ui/src/lib/generated/bindings.ts`                           | 型の再生成                                                  |

## 6. 次のフェーズ

- Phase 2: `AgentExtension` / `ExtendedAcpDriver` / `CapabilityOverlay` と
  `DriverKind::AcpExtended`
- Phase 3: `DriverKind::Custom` と full custom driver の登録経路
- Phase 4: `opencode` の Tier 2 移行と `CustomAgentConfig.extension`
