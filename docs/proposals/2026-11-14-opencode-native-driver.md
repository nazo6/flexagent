# OpenCode v2 ネイティブドライバ (Tier 3)

- **日付**: 2026-11-14
- **状態**: 実装済み
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-hub`, `fxg-node`, `ui`,
  `docs`
- **対象スクリプト**: `mise run fmt`, `mise run lint:errors`,
  `mise run types`, `cargo test --workspace`, `pnpm --dir ui run check`,
  `pnpm --dir ui run test`

## 1. 背景

OpenCode の ACP 実装は `session/list` / `resume` / `fork` / MCP など多くの
機能を提供するが、OpenCode v2 の HTTP API が持つ次の操作を持たない。

- 会話の revert（native `revert/stage` + `revert/commit`）
- メッセージ境界からの fork（`fork { before }`）
- 実行中の入力（`delivery: steer` / inbox）
- provider / credential / MCP / skill / plugin / config の管理
- セッションの agent / model の即時切替と variant

これらは OpenChamber が `@opencode/client` 経由で提供している範囲でもある。
本提案では OpenCode を **Tier 3 (`impl AgentDriver`)** に昇格し、OpenCode v2
の HTTP + SSE を直接扱う `OpenCodeNativeDriver` を追加した。ACP 経路は
`opencode-acp` として残す。

## 2. 構成

- `crates/fxg-acp/src/opencode_native.rs`
  - `opencode serve` のサービス探索 (`<state>/opencode/service.json`) と、
    不在時の専用サーバー起動
  - HTTP クライアント（Basic auth）
  - `/api/event` の SSE 購読とセッション別 fan-out
  - セッションごとの runtime（busy / tool name / self message / durable seq）
  - `AgentDriver` / `ActiveSessionHandle` 実装
  - 管理 snapshot と MCP 操作
- `crates/fxg-acp/src/agent/builtin.rs`
  - `opencode` = Tier 3 ネイティブ
  - `opencode-acp` = 従来の ACP + `OpenCodeExtension`
- `crates/fxg-protocol`
  - `UserMessage.agent_message_id`
  - `SessionControlAction::{Revert, Compact}`
  - `UnifiedEventPayload::SessionReverted`
  - `NativeSessionFeature` と `CapabilitiesUpdated.native_features`
  - `OpenCodeManagementSnapshot` と管理型
- `crates/fxg-hub` / `crates/fxg-node`
  - `GET /api/v1/nodes/:id/opencode`（ローカルノードのみ）
  - `PUT/DELETE /api/v1/nodes/:id/opencode/mcp/:name`
  - `POST /api/v1/nodes/:id/opencode/mcp/:name/{connect,disconnect}`
- `ui`
  - メッセージ境界の Revert、revert 区間のタイムライン非表示
  - ネイティブ機能の capability 表示
  - Settings > Node > OpenCode（agents / models / providers / MCP / skills /
    commands）

## 3. データ所有権

OpenCode のセッションを一次ソースとし、fxg の `node.db` には正規化イベント
（およびハブ同期用の投影）を記録する。ACP と同じく、OpenCode の TUI や
OpenChamber が同じセッションを操作した場合、`/api/event` の
`session.inbox.enqueued` / `session.text.*` が fxg に流れ、タイムラインへ
反映される。

Revert は fxg のイベントログを物理削除せず、`SessionReverted` を境界として
クライアントが区間を非表示にする。検索インデックスには残るため、削除済み
会話の扱いは将来の課題とする。

## 4. 既知の制約

- `opencode serve` の専用起動時、Unix ではプロセスグループ終了を保証しない
  （Windows は Job Object）。`kill_on_drop` で直接の子は終了する。
- ノード停止中に TUI で進めた会話は、再開時に自動では取り込まない。
  明示 import は `capture_history` で履歴を mirror する。
- provider の資格情報は読み取り専用で、UI からは変更しない（secret を
  hub に同期しない方針）。
- OpenCode の HTTP API は experimental のため、バージョンゲートと mock
  契約テストで追従する。

## 5. 検証

- `crates/fxg-acp/tests/opencode_native_mock.rs`: HTTP/SSE、permission、
  form、subagent、revert、fork、compact の契約テスト
- `crates/fxg-acp/tests/opencode_native_real.rs` (`#[ignore]`): 実 OpenCode
  で prompt ストリーミング、revert、fork を検証
- `crates/fxg-node/src/session_manager/tests.rs`: steering、revert の
  message ID 解決、メッセージ境界 fork、compact 制御
- 手動 E2E: 一時 `FXG_HOME` で `fxg session new opencode` →
  `NATIVE_OK` 受信、`/control` で revert、`fxg session fork` を確認
