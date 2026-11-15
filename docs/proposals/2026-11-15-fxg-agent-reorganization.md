# `fxg-agent` のモジュール構成・トレイト・責務境界の再編

- **日付**: 2026-11-15
- **状態**: Draft
- **対象パッケージ**: `fxg-agent`, `fxg-node`, `fxg-cli`, `docs`
- **対象スクリプト**: `mise run fmt`, `mise run lint:errors`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`

---

## 1. 背景と課題

`fxg-acp` から `fxg-agent` へのリネーム（2026-11-15）および `OpenCodeNativeDriver`（Tier 3）の追加を経た結果、現在の `crates/fxg-agent/src/` には歴史的経緯による責務のねじれと構造的な肥大化が残っている。

1. **ACP 固有コードのクレート直下への混在**:
   - `auth.rs`（ACP `initialize`/`authenticate`/`logout` と汎用コマンド実行が混在）、`warm.rs`（`acp/` 専用）、`registry.rs`（ACP Registry 取得と `AgentCatalogState` 所有が混在）、`extension.rs`（`AcpExtensionOptions` / `AcpNotificationParser` 等 ACP 型に依存）がクレート直下に並んでいる。
2. **Tier 2 (`AgentExtension` / `ExtendedAcpDriver`) の過剰抽象化**:
   - 当初 OpenCode Tier 2 向けに導入された `CapabilityOverlay`、`ConfigOptionPatch`、`CapabilitiesSnapshot`、`SessionExtension`（`before_prompt` / `before_config` / `shutdown`）、`prepare_launch` は本番実装で未使用（テストのみ）であり、実利用されているのは `OpenCodeExtension`（`opencode-acp` フォールバック）の 4 点（`metadata`、`acp_options`、`notification_parser`、イベントの `subagent_id` タグ付け）に限られる。
   - イベント変換のために `ExtendedAcpDriver`（689行）が別タスクと `tap_tx` チャネルを挟んで中継している。
3. **`AgentProvider` トレイトの形骸化**:
   - `AgentProvider` の実装は `StaticProvider` のみであり、動的な振る舞いはすでに `AgentDriver` / `AgentExtension` / `AgentManagement` / `AgentAuth` に移っている。
4. **`AgentCatalog` と `AcpRegistry` の所有権の反転**:
   - `fxg-node` / `fxg-cli` ともに `AcpRegistry::new(...)` を作って即 `AgentCatalog::new(...)` に渡しているが、内部では `AcpRegistry` が `node.db` 由来の `AgentCatalogState`（`custom` / `aliases`）を所有し、`AgentCatalog` がそこへ委譲する反転構造になっている。
5. **共通起動仕様への ACP 型の漏れ**:
   - `driver.rs` の `AgentLaunchSpec` が ACP 固有の `extension` と `acp: AcpExtensionOptions` を直接保持しており、`DriverKind` も `fxg_protocol::client_api::AgentBackendKind` と責務が重複している。
6. **特定モジュールの肥大化とテスト配置の不統一**:
   - `opencode_native/mapping.rs`（2038行）、`acp/session.rs`（1170行）、`catalog.rs`（1066行）、`acp/client.rs`（1011行）、`acp/mapping.rs`（976行）、`registry.rs`（949行）が肥大化し、テスト配置もインライン `#[cfg(test)]` / `tests.rs` / `*_tests.rs` が混在している。

---

## 2. 設計方針

### 2.1 モジュール・ディレクトリ構成（目標像）

クレート直下は「共通契約 + カタログ + エージェント実装ディレクトリ」に純化し、ACP 固有機能は `acp/` 配下へ集約する。OpenCode ネイティブドライバは `opencode_native/` から `opencode/` へリネームし、Tier 2 の `OpenCodeExtension` は ACP 側の拡張として `acp/extensions/` へ移す。

```text
crates/fxg-agent/src/
├── lib.rs                      # 明確なファサード（pub use と最小限の pub mod のみ）
├── driver.rs                   # AgentDriver / ActiveSessionHandle / DriverEvent / StartSessionRequest / エラー型
├── management.rs               # AgentManagement トレイト
├── auth.rs                     # 汎用認証コマンド実行ヘルパー (run_agent_command 等) と共通認証型
├── catalog/
│   ├── mod.rs                  # AgentCatalog（custom/aliases 状態所有・一覧・launch_spec・driver_for・auth/management 解決）
│   ├── builtin.rs              # BuiltinAgent 構造体・AgentBackend・LaunchPreset・ビルトイン定義
│   └── catalog_tests.rs        # AgentCatalog のユニットテスト
├── acp/
│   ├── mod.rs                  # AcpDriver（extension 直接適用）、StderrRegistry
│   ├── auth.rs                 # ACP initialize / authenticate / logout / terminal login
│   ├── warm.rs                 # pub(super) WarmPool / LaunchKey
│   ├── registry/
│   │   ├── mod.rs              # AcpRegistry（registry.json 取得・キャッシュ・import 変換のみ）
│   │   ├── types.rs            # RegistryIndex / RegistryAgent / RegistryDistribution 等
│   │   └── registry_tests.rs   # AcpRegistry のユニットテスト
│   ├── extensions/
│   │   ├── mod.rs              # スリム化した AgentExtension / AcpExtensionOptions / AcpNotificationParser
│   │   └── opencode.rs         # OpenCodeExtension (opencode-acp フォールバック用)
│   ├── client.rs               # AcpSessionHandle / acp::Client 実装
│   ├── session/
│   │   ├── mod.rs              # セッション構築・ターンループ
│   │   └── capabilities.rs     # capabilities probe / mode・config option 適用
│   ├── mapping/
│   │   ├── mod.rs              # session_update 正規化・ストリーム蓄積
│   │   └── types.rs            # enum / 構造体マッピング
│   ├── list.rs
│   ├── process.rs
│   ├── subagent.rs
│   ├── terminal.rs
│   └── tests.rs                # acp ユニットテスト
└── opencode/                   # 旧 opencode_native/ をリネーム・分割
    ├── mod.rs
    ├── auth.rs                 # OpenCode ネイティブ認証定義 (opencode auth login/logout)
    ├── driver.rs               # OpenCodeNativeDriver / ActiveSessionHandle 実装
    ├── server.rs               # opencode serve プロセス起動・service.json 検出
    ├── client.rs               # OpenCodeClient (HTTP + SSE)
    ├── state.rs                # mapping.rs から分離: SessionRuntime / RuntimeState / SeenEventIds / UsageState
    ├── mapping.rs              # SSE・API レスポンス → DriverEvent / Capabilities 変換
    ├── history.rs              # mapping.rs から分離: 履歴インポート・revert/fork 補助
    ├── client_tests.rs
    └── driver_tests.rs
```

### 2.2 トレイトと型の再設計

1. **`AgentDriver` / `ActiveSessionHandle`（フラット構成の維持と微小整理）**
   - 現行のフラットなトレイト粒度を維持する（`fxg-node` の `SessionManager` との境界が安定しているため、不要なサブトレイト分割・ダウンキャストは導入しない）。
   - `AgentDriver` から外部未参照の `stderr_tail(&self, agent_id: &str)` を削除し、`stderr_snapshot(&self)` に一本化する。
2. **`AgentLaunchSpec` と `LaunchBackend` の分離**
   - `AgentLaunchSpec` からトップレベルの ACP 固有フィールド（`extension`, `acp: AcpExtensionOptions`）を取り除き、バックエンド固有情報（`LaunchBackend::Acp(AcpLaunchOptions)` / `LaunchBackend::Custom` / `LaunchBackend::Other(String)` 等）へ構造化する。
   - `DriverKind` との重複を解消し、プロトコル型 `AgentBackendKind` との対応を明確にする。
3. **Tier 2 (`AgentExtension`) のスリム化と `ExtendedAcpDriver` の廃止**
   - `extended.rs`（`ExtendedAcpDriver` とバックグラウンド中継タスク）を廃止し、`AcpDriver` が登録された `Arc<dyn AgentExtension>` を直接受け取って `notification_parser` およびイベント変換を適用する。
   - `AgentExtension`（`acp::extensions` 配下へ移動）から本番未使用のフック・型（`CapabilityOverlay`, `ConfigOptionPatch`, `CapabilitiesSnapshot`, `SessionExtension`, `Hook`, `SessionContext`, `capabilities()`, `open_session()`, `prepare_launch()`）を削除し、実利用される以下に絞る：
     - `metadata(&self) -> AgentMetadata`
     - `acp_options(&self, capabilities: &AgentAcpCapabilities) -> AcpExtensionOptions`
     - `notification_parser(&self) -> Option<Arc<dyn AcpNotificationParser>>`
     - `transform_event(&self, event: DriverEvent) -> EventDisposition`（所有権を受け取る同期変換とし、全イベントの不要な `clone()` とチャネル中継をなくす）
   - `AgentBackendKind::AcpExtended` や `CustomAgentConfig.extension` の DB / API / UI 互換はそのまま維持する。
4. **`AgentProvider` トレイトの廃止**
   - `provider.rs` の `AgentProvider` トレイトを削除し、`StaticProvider` を具象構造体 `BuiltinAgent`（`definition`, `preset`, `backend: AgentBackend`, `auth: Option<AgentAuth>`）へ置き換える。
   - `AgentCatalog::with_agents(...)`（旧 `with_providers`）でテスト用のカスタム一覧を受け取れるようにし、トレイトによる不要な間接参照をなくす。
5. **`AgentCatalog` と `AcpRegistry` の責務分離**
   - `AgentCatalog` が `custom` 定義・`aliases`（`AgentCatalogState`）とビルトイン定義・拡張・ドライバ解決を直接所有する。
   - `AcpRegistry` は `registry.json` の HTTP 取得・ローカルキャッシュ（`<cache>/node/registry.json`）・import 用 `CustomAgentConfig` 生成のみに責務を限定する。`AcpRegistry::new` から `custom` 引数を外し、`registry_url` / `cache_ttl_secs` を受け取る（または更新メソッドで反映する）純粋なクライアントにする。
6. **`auth` の責務分離**
   - `AgentAuth` / `AgentCommand` と汎用コマンド実行（`run_agent_command`）を `auth.rs`（または `catalog` と共通層）に残し、ACP 専用の `initialize` / `authenticate` / `logout` / `run_terminal_login` を `acp::auth` へ移す。
7. **`lib.rs` ファサードと互換ポリシー**
   - 過剰な後方互換エイリアスやリダイレクトモジュールは残さず、`fxg-node`、`fxg-cli`、および `crates/fxg-agent/tests/` の呼び出し箇所を同時に更新する。

### 2.3 テスト配置ルールと内部スタイル

- **テスト配置の統一**:
  - 小規模テスト（概ね 200 行未満）は対象ファイル内 `#[cfg(test)] mod tests` に置く。
  - 大規模テスト（`catalog`, `acp/registry`, `acp`, `opencode/client`, `opencode/driver`）は各モジュール配下の `*_tests.rs` または `tests.rs` に置く。
- **肥大化モジュールの分割**:
  - `opencode/mapping.rs`（2038行）からランタイム状態 (`state.rs`) と履歴・revert 系変換 (`history.rs`) を分離する。
  - `acp/session.rs`（1170行）と `acp/mapping.rs`（976行）もセッション制御 / probe・capability 適用 / 型変換の単位で分割する。

---

## 3. 段階的実装計画

破壊的変更の影響範囲を検証しながら安全に進めるため、以下の 3 フェーズで実施する。

- **Phase 1: Tier 2 スリム化と `ExtendedAcpDriver` 統合**
  - `AgentExtension` から未使用フック・`CapabilityOverlay`・`SessionExtension` を削除。
  - `AcpDriver` へ拡張のイベント変換 (`transform_event`) を直接組み込み、`extended.rs` を削除。
  - `fxg-node/src/session_manager/tests.rs` の該当テスト（`CapabilityOverlay` 依存箇所）と `crates/fxg-agent/tests/subagents.rs` を更新。
- **Phase 2: `AgentProvider` 廃止・`AgentCatalog` / `AcpRegistry` 分離・`AgentLaunchSpec` 整理**
  - `AgentProvider` トレイトを廃止し、`BuiltinAgent` 構造体へ移行。
  - `AgentCatalogState` の所有を `AcpRegistry` から `AgentCatalog` へ移し、`AcpRegistry` を `acp/registry/` へ純化。
  - `AgentLaunchSpec` の `LaunchBackend` 化と `AgentDriver::stderr_tail` の削除。
  - `fxg-node`（`daemon/mod.rs`, `session_manager/`, `testutil.rs`）と `fxg-cli`（`commands/agents.rs`）の呼び出しを更新。
- **Phase 3: モジュール再編・肥大化ファイル分割・ドキュメント昇格**
  - `auth.rs` / `warm.rs` / `extension.rs` / `opencode.rs` を `acp/` 配下へ、`opencode_native/` を `opencode/` へ移動・リネーム。
  - `opencode/mapping.rs`・`acp/session.rs`・`acp/mapping.rs`・`catalog` の分割とテスト配置統一。
  - `lib.rs` のファサード整理。
  - [`docs/04-agent-drivers-and-windows.md`](../04-agent-drivers-and-windows.md) §1.1 の責務・不変条件を更新し、本提案を `Implemented` へ昇格。

---

## 4. 検証項目

各フェーズ完了後に以下がすべて通ることを確認する。

- `mise run fmt`
- `mise run lint:errors`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
