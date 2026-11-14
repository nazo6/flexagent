# Tier 2 拡張基盤（3層モデル Phase 2）

- **日付**: 2026-11-11
- **状態**: 実装済み（Phase 2 / 既定の挙動は ACP only のまま）
- **対象パッケージ**: `fxg-acp`, `fxg-node`, `fxg-cli`, `docs`
- **対象スクリプト**: `mise run fmt`, `mise run lint:errors`,
  `cargo test -p fxg-acp -p fxg-node -p fxg-cli`,
  `cargo test -p fxg-protocol -p fxg-hub`

> 3層モデル全体の設計は
> [`docs/changelog/2026-11-11-agent-extension-architecture.md`](../../proposals/2026-11-11-agent-extension-architecture.md)
> を参照。本ファイルは Phase 2 の実装記録です。

---

## 1. 目的

Tier 2（ACP + fxg-side extension）の基盤を追加する。ACP の起動・イベント・
権限・resume はそのまま使い、メタデータ・起動引数・能力情報・セッション操作の
差分だけを `AgentExtension` に実装できるようにする。

Phase 2 ではビルトインエージェントを Tier 2 に移行しないため、既存の ACP only
経路の挙動は変わらない。最初の実例は Phase 4（opencode）で追加する。

## 2. 変更内容

### 2.1 `AgentExtension` / `SessionExtension`

`crates/fxg-acp/src/agent/extension.rs` を追加した。

- `AgentExtension`:
  - `metadata()`: 表示名・説明・アイコンの上書き
  - `prepare_launch()`: 起動 args / env の調整（cwd を受け取る）
  - `capabilities()`: 現在の能力情報（`CapabilitiesSnapshot`）を受け取り
    `CapabilityOverlay` を生成
  - `open_session()`: セッション固有の `SessionExtension` を返す
- `SessionExtension`:
  - `before_prompt()`: プロンプトの前処理（`Handled` で ACP をスキップ）
  - `before_config()`: 設定変更の介入
  - `on_event()`: イベントの観測・置換・破棄
  - `shutdown()`: セッション固有リソースの解放
- `CapabilityOverlay` / `ConfigOptionPatch`:
  - modes / current mode の置換
  - config option の `Replace` / `Merge` / `Remove`
  - slash command の追加
- extension インスタンスはセッション間で共有される。セッション固有の状態は
  `SessionExtension` に置く。

### 2.2 `ExtendedAcpDriver`

`crates/fxg-acp/src/agent/extended.rs` を追加した。

- `AgentDriver` を実装し、`driver_kind()` は `DriverKind::AcpExtended`
- `start_session` で ACP ドライバのイベントを tap し、relay タスクを起動する
- relay は `CapabilitiesUpdated` に `CapabilityOverlay` を適用し、その後
  `SessionExtension::on_event` に通す
- overlay は `CapabilitiesUpdated` ごとに現在値から再計算する
  （現在値に基づく `Replace` が古い値で上書きしないため）
- overlay / `on_event` の失敗はセッションを落とさず、元の ACP イベントを
  そのまま流す
- `ActiveSessionHandle` は ACP ハンドルへ委譲し、`send_prompt` /
  `set_config` / `shutdown` の前後に session extension を呼ぶ

### 2.3 driver / catalog

- `DriverKind::AcpExtended` を追加した
- `StartedSession.agent_session_id` を追加し、`AcpDriver` が ACP セッション ID を
  設定する
- `AgentBackend::AcpExtended(Arc<dyn AgentExtension>)` を追加した
- `StaticProvider::with_backend()` で Tier 2 provider を構築できるようにした
- `AgentCatalog`:
  - `launch_spec()` がセッションの cwd を受け取り、Tier 2 の
    `prepare_launch()` を適用する
  - provider backend が拡張なら `DriverKind::AcpExtended` を設定する
  - `driver_for()` が `ExtendedAcpDriver` を生成し、エージェント単位でキャッシュする
  - `list()` が extension の metadata を一覧へ反映する

### 2.4 `fxg-node` / `fxg-cli`

- `SessionManager::resolve_launch_spec()` がセッションの cwd を
  `AgentCatalog::launch_spec()` へ渡す
- `fxg agents login/logout` はカレントディレクトリを cwd として渡す
- `SessionManager` のイベント処理・probe の構造は変更しない

## 3. 挙動

- 既定のビルトインエージェント（`opencode` / `agy`）は引き続き Tier 1
- Tier 2 provider を登録しない限り、`AcpExtended` は使われない
- extension の失敗はログに留め、ACP の処理を継続する
- capability probe は通常セッションと同じ relay を通るため、上書き結果が
  `AgentCapabilitiesPreview` に反映される

## 4. テスト

- `AgentExtension` / `CapabilityOverlay`:
  - modes / commands の適用
  - config option の merge / replace / remove
- `ExtendedAcpDriver`:
  - overlay と session hook の適用
  - overlay 失敗時に元のイベントが流れること
  - `agent_session_id` が context に渡ること
- `AgentCatalog`:
  - Tier 2 provider の `DriverKind::AcpExtended` 選択
  - `ExtendedAcpDriver` のキャッシュ
  - `prepare_launch` と metadata の反映
- `fxg-node`:
  - capability probe が extension の上書き結果を返すこと
  - 既存の session manager / sync E2E テストが回帰しないこと

## 5. 変更ファイル

| ファイル                                                   | 変更内容                                                      |
| :--------------------------------------------------------- | :------------------------------------------------------------ |
| `crates/fxg-acp/src/agent/extension.rs`                    | extension / session hook / capability overlay の型            |
| `crates/fxg-acp/src/agent/extended.rs`                     | `ExtendedAcpDriver` とイベント relay                          |
| `crates/fxg-acp/src/agent/provider.rs`                     | `AgentBackend::AcpExtended` / `StaticProvider::with_backend`  |
| `crates/fxg-acp/src/agent/catalog.rs`                      | cwd 付き `launch_spec`、Tier 2 の driver / metadata 合成      |
| `crates/fxg-acp/src/agent/mod.rs`                          | 新モジュールの公開                                            |
| `crates/fxg-acp/src/driver.rs`                             | `DriverKind::AcpExtended` / `StartedSession.agent_session_id` |
| `crates/fxg-acp/src/acp/{mod,client}.rs`                   | ACP セッション ID の取得                                      |
| `crates/fxg-node/src/session_manager/{lifecycle,probe}.rs` | `launch_spec` へ cwd を渡す                                   |
| `crates/fxg-node/src/session_manager/tests.rs`             | probe overlay の統合テスト                                    |
| `crates/fxg-cli/src/commands/agents.rs`                    | login/logout で cwd を渡す                                    |
| `crates/fxg-node/src/testutil.rs`, `tests/sync_e2e.rs`     | `StartedSession.agent_session_id` の追加                      |

## 6. 次のフェーズ

- Phase 3: `DriverKind::Custom` と full custom driver の登録経路
- Phase 4: `opencode` の Tier 2 移行と `CustomAgentConfig.extension`
