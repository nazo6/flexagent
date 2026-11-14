# Fork 先への会話履歴の複製

- **日付**: 2026-11-12
- **対象パッケージ**: `fxg-protocol` / `fxg-db` / `fxg-node` / `fxg-acp` / `docs`
- **対象スクリプト**: `cargo test -p fxg-protocol -p fxg-db -p fxg-node`,
  `cargo clippy --workspace --all-targets`, `cargo fmt --all --check`

## 背景

ACP ネイティブ Fork はエージェント側の会話を引き継ぐが、fxg の新セッションの
イベントログは空のままだった。`session/load` のリプレイは Resume との二重記録を
避けるため破棄しており、Fork では表示できる履歴が存在しなかった。結果として
Web UI / CLI では「Fork しても前の会話が引き継がれていない」ように見えていた。

## 変更内容

- `UnifiedEventPayload::is_fork_copyable` を追加し、会話イベント
  (`UserMessage` / `AgentMessage` / `AgentThought` / `ToolCall` / `PlanUpdate` /
  `TerminalOutput` / `SubagentUpdated` / `SubagentMessage` / `TurnEnded`)
  だけを複製対象とした。セッション固有のメタイベント（作成・タイトル・状態・
  能力・承認・利用量）は複製しない。
- `fxg-db::events::copy_fork_conversation` を追加。Fork 元のイベントを Fork 先の
  新しい `node_seq` / `event_id` で単一トランザクションで追記し、`created_at`
  は元の時刻を保持する。複製範囲は Fork 作成時点の `last_node_seq` まで。
- `SessionEventBus::copy_conversation_events` で複製したイベントを WS /
  Outbox に配信する。
- `SessionManager::start_session_inner` がドライバ起動後・最初のプロンプト送信前に
  親の会話を複製する。`fork` / REST / Hub 経由のすべての Fork が同じ経路を通る。
- `session/load` のリプレイを破棄する理由のコメントを更新（Fork は親の
  ログを複製するため）。

## 互換性

- イベント型・DB スキーマは変更しない（既存イベントの再解釈のみ）。
- 既存の Fork セッションには遡って履歴を追加しない。

## 検証

- `cargo test -p fxg-protocol fork_copy`
- `cargo test -p fxg-db copy_fork_conversation`
- `cargo test -p fxg-node fork`
  (`fork_uses_native_session_fork` / `start_session_with_fork_from_uses_native_fork`)
- 実機: opencode 2.2.7 の ACP で別プロセスから `session/fork` +
  `session/load` し、Fork 先が親の会話を継承することを確認済み。
