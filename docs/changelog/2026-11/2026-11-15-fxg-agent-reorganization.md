# `fxg-agent` のモジュール再編と Tier 2 のスリム化

- **日付**: 2026-11-15
- **状態**: 実装済み
- **対象パッケージ**: `fxg-agent`, `fxg-node`, `fxg-cli`, `docs`
- **対象スクリプト**: `mise run fmt`, `mise run lint:errors`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`

設計判断と段階計画は
[`docs/proposals/2026-11-15-fxg-agent-reorganization.md`](../../proposals/2026-11-15-fxg-agent-reorganization.md)
を参照。本ファイルは実装結果の記録です。

---

## 1. 目的

`fxg-acp` → `fxg-agent` のリネームと OpenCode ネイティブドライバ (Tier 3) の
追加で残った責務のねじれを解消する。ACP 固有コードを `acp/` に集約し、
Tier 2 の未使用抽象化とトレイトの間接参照を削り、`AgentCatalog` と
`AcpRegistry` の所有権を正す。

## 2. 変更内容

### 2.1 Tier 2 のスリム化

- `AgentExtension` を `metadata` / `acp_options` / `notification_parser` /
  `transform_event` の 4 点に絞った。未使用だった `CapabilityOverlay` /
  `ConfigOptionPatch` / `CapabilitiesSnapshot` / `SessionExtension` /
  `prepare_launch` を削除した。
- `ExtendedAcpDriver` とその中継タスクを廃止した。`AcpDriver` が extension を
  直接保持し、`EventSink` が `DriverEvent` を所有権ごと `transform_event` に
  渡してからセッション管理へ転送する。イベントの clone とチャネル中継がなく
  なった。
- OpenCode 拡張の子セッション ID 付与を `transform_event` に移した。

### 2.2 トレイトと起動スペック

- `AgentProvider` / `StaticProvider` を廃止し、具体構造体 `BuiltinAgent` に
  置き換えた。`definition` / `preset` / `backend` / `auth` を所有し、
  `with_backend` / `with_extension` / `with_auth` で組み立てる。
- `AgentLaunchSpec` の `driver_kind` / `extension` / `acp` を
  `LaunchBackend::Acp(AcpLaunchOptions)` / `LaunchBackend::Custom` /
  `LaunchBackend::Other` に統合した。`DriverKind` は `AgentDriver` の
  ログ用識別子だけに残る。
- 外部から参照されていなかった `AgentDriver::stderr_tail` を削除し、
  `stderr_snapshot` に一本化した。

### 2.3 カタログとレジストリ

- `AgentCatalogState`（aliases / custom 定義 / registry 設定）の所有を
  `AcpRegistry` から `AgentCatalog` へ移した。
- `AcpRegistry` は `registry.json` の取得・キャッシュ・import 変換だけを行う
  クライアントになり、`configure(registry_url, cache_ttl_secs)` で設定を受け取る。
- `custom_launch_spec` / `custom_entries` / `resolve_alias` は `AgentCatalog` へ
  移動した。

### 2.4 モジュール構成

- ACP 固有コードを `acp/` 配下へ移動した: `acp::auth`、`acp::warm`、
  `acp::registry`（`types` / `registry_tests`）、`acp::extensions`
  （`mod` / `opencode`）、`acp::session`（`mod` / `capabilities`）。
- `acp::mapping` を `mod` + `types` に分割した。
- `catalog` を `mod` + `builtin` + `catalog_tests` に分割した。
- `opencode_native/` を `opencode/` へリネームし、`mapping` から
  `state`（セッションランタイム状態）と `history`（履歴リプレイ）を分離した。
- `lib.rs` は `pub mod` を `acp` / `auth` / `catalog` / `driver` /
  `management` / `opencode` に絞り、後方互換のエイリアスを残さない。

## 3. 影響と移行

- `fxg-node` は `AgentCatalog::new(paths, &settings, custom)` を使い、
  `AcpRegistry` を直接組み立てない。
- `fxg-cli` の ACP 認証は `fxg_agent::acp::auth` を参照する。
- `AgentLaunchSpec` を構築するテストは `backend: LaunchBackend::...` を設定する。
- DB スキーマ (`custom_agents.extension` / `CapabilitiesUpdated`) と
  Web UI の API は変更していない。
