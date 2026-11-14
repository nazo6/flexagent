# Full custom driver 基盤（3層モデル Phase 3）

- **日付**: 2026-11-11
- **状態**: 実装済み（Phase 3 / 既定の挙動は ACP only のまま）
- **対象パッケージ**: `fxg-acp`, `fxg-node`, `fxg-cli`, `docs`
- **対象スクリプト**: `mise run fmt`, `mise run lint:errors`,
  `cargo test -p fxg-acp -p fxg-node -p fxg-cli`

> 3層モデル全体の設計は
> [`docs/changelog/2026-11-11-agent-extension-architecture.md`](../../proposals/2026-11-11-agent-extension-architecture.md)
> を参照。本ファイルは Phase 3 の実装記録です。

---

## 1. 目的

ACP に依存しない full custom driver（Tier 3）を、`AgentProvider` から登録して
`AgentCatalog` / `SessionManager` 経由で起動できるようにする。Phase 3 では実例を
追加しないため、既定のエージェントは引き続き ACP only のままである。

## 2. 変更内容

### 2.1 `AgentLaunchSpec.program` の `Option` 化

- `AgentLaunchSpec.program` を `Option<PathBuf>` に変更した
- ACP ドライバ（`AcpDriver` / 認証処理）は `Some` を必須とし、`None` の場合は
  明確なエラーを返す
- ローカルプロセスを起動しない full custom driver は `None` を受け取れる
- `LaunchKey`（warm pool）は従来どおり解決済みの `PathBuf` を使う

### 2.2 `AgentBackend::Custom`

- `AgentBackend::Custom(Arc<dyn AgentDriver>)` を追加した
- `StaticProvider::with_backend()` で Tier 3 provider を構築できる
- `AgentProvider::backend()` の `kind()` は `custom` を返す

### 2.3 catalog / driver 選択

- `DriverKind::Custom` を追加した
- `AgentCatalog::driver_for()` が provider の driver clone を返す
- `AgentCatalog::launch_spec()` が custom driver 用の spec を生成する
  - `program: None`
  - pass-through 引数は保持する
  - provider に preset があり `PATH` 上で見つかる場合は `program` / `args` を埋める
- `AgentCatalog::list()`:
  - Tier 3 provider は preset がなくても一覧へ出す
  - `AgentListEntry::is_available()` は `backend == custom` を利用可能と判定する
  - Tier 3 provider の registry entry は重複表示しない

### 2.4 `AgentDriver` の契約

`AgentDriver` の doc コメントに full custom driver の契約を明記した。

- driver はデーモン全体で共有されるため `Send + Sync` とし、セッション状態は
  `ActiveSessionHandle` に置く
- `start_session` は正規化された `DriverEvent` を送る
- `context_restored` はネイティブ復元できた場合だけ `true`
- `agent_session_id` はネイティブ復元できる場合に設定する
- `AgentLaunchSpec.program` は `None` を許容する
- `shutdown` / `dispose` の責務

## 3. テスト

- `fxg-acp`:
  - `program: None` の Tier 3 spec と `DriverKind::Custom`
  - preset がある場合の `program` / `args`
  - `AgentCatalog::driver_for` が provider の driver を返すこと
  - `sources` が空でも一覧で利用可能と判定されること
- `fxg-node`:
  - `SessionManager::new`（catalog factory）経由で full custom driver を起動し、
    `ensure_session` が成功すること
  - 同じ経路の capability probe が driver の能力を返すこと
  - 既存の session manager / sync E2E テストが回帰しないこと

## 4. 変更ファイル

| ファイル                                         | 変更内容                                                   |
| :----------------------------------------------- | :--------------------------------------------------------- |
| `crates/fxg-acp/src/driver.rs`                   | `DriverKind::Custom`、`program: Option`、driver 契約の doc |
| `crates/fxg-acp/src/acp/{mod,client}.rs`         | ACP は `program: Some` を必須化                            |
| `crates/fxg-acp/src/auth.rs`                     | 認証経路の `program` unwrap                                |
| `crates/fxg-acp/src/agent/provider.rs`           | `AgentBackend::Custom`                                     |
| `crates/fxg-acp/src/agent/catalog.rs`            | Tier 3 の spec / driver 選択 / 一覧                        |
| `crates/fxg-acp/src/registry.rs`                 | `AgentListEntry::is_available` の custom 対応              |
| `crates/fxg-acp/src/agent/{extended,builtin}.rs` | `program: Option` への追従                                 |
| `crates/fxg-acp/tests/*`                         | `program: Some` への追従                                   |
| `crates/fxg-node/src/session_manager/tests.rs`   | full custom driver の起動テスト                            |

## 5. 次のフェーズ

- Phase 4: `opencode` の Tier 2 移行と `CustomAgentConfig.extension`
- Phase 5: ドキュメントの最終更新
