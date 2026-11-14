# OpenCode v2 ネイティブドライバ (Tier 3) と復旧・管理 API

- **日付**: 2026-11-14
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-hub`, `fxg-node`, `ui`,
  `docs`
- **対象スクリプト**: `mise run fmt`, `mise run lint:errors`,
  `mise run types`, `cargo test --workspace`, `pnpm --dir ui run check`,
  `pnpm --dir ui run test`

OpenCode を Tier 3（full custom driver）に移行し、OpenCode v2 の HTTP + SSE
を直接扱う `OpenCodeNativeDriver` を追加した。従来の ACP 経路は
`opencode-acp` として残した。

## 実装

- **ドライバ**: `crates/fxg-acp/src/opencode_native.rs`
  - 実行中サービスの探索、専用 `opencode serve` の起動、SSE 再接続と
    durable log の backfill
  - セッション作成 / bind / resume / fork、prompt、command、attachment、
    interrupt、agent / model 切替、compact、revert
  - text / reasoning / tool / permission / form / usage / compaction /
    subagent / title の正規化
  - `delivery: steer` による実行中入力、`SessionAgentBound` / capabilities
- **Tier 3 登録**: `opencode` はネイティブ、`opencode-acp` は ACP。
  `AgentCatalog` の `AgentBackend::Custom` 経由で選択する。
- **プロトコル**:
  - `UserMessage.agent_message_id`
  - `SessionControlAction::{Revert, Compact}`
  - `UnifiedEventPayload::SessionReverted`
  - `NativeSessionFeature` / `CapabilitiesUpdated.native_features`
  - `OpenCodeManagementSnapshot` ほか管理型
- **ノード / ハブ**:
  - `GET /api/v1/nodes/:id/opencode`
  - `PUT/DELETE /api/v1/nodes/:id/opencode/mcp/:name`
  - `POST /api/v1/nodes/:id/opencode/mcp/:name/{connect,disconnect}`
  - ローカルノード限定（ハブの backend は明示エラー）
- **UI**:
  - メッセージ境界の Revert と、revert 区間のタイムライン非表示
  - Settings > Node > OpenCode（agents / models / providers / MCP /
    skills / commands、MCP の追加・接続・削除）
  - `native_features` に応じた Revert アクションの表示

## テスト

- `opencode_native_mock`: モック HTTP/SSE サーバーで prompt、permission、
  form、subagent、revert、fork、compact を検証。
- `opencode_native_real` (`#[ignore]`): 実 OpenCode v2.2.7 で prompt
  ストリーミング、revert、fork を検証。
- `fxg-node` session manager: steering、revert、message-boundary fork、
  compact。
- UI: revert 区間のタイムライン非表示と capabilities の native features。

## 手動確認 (2026-11-14)

- 一時 `FXG_HOME` + 一時 Git リポジトリで `fxg session new opencode` を実行し、
  `agent_bound` / `capabilities_updated` / `agent_message NATIVE_OK` /
  `usage` / `status_changed idle` を確認。
- ローカル REST `/api/v1/sessions/:id/control` の `revert` で
  `session_reverted up_to=5` を確認。
- `fxg session fork` で新しい OpenCode セッション ID と履歴 mirror を確認。
- `/api/v1/nodes/:id/opencode` が OpenCode 2.2.7 の agents / models /
  providers / MCP / skills / commands を返すことを確認。

## 制約

- Unix の専用サーバーはプロセスグループ終了を保証しない（Windows は Job
  Object）。ノード停止中の TUI 変更は自動 backfill しない。
- OpenCode API は experimental。バージョンゲートと mock 契約テストで追従する。
