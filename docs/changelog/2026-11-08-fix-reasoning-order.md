# 2026-11-08 思考 (reasoning) が回答の後に表示される問題の修正

- **日付**: 2026-11-08
- **対象パッケージ**: `fxg-acp`, `ui`
- **対象スクリプト**: `mise run fmt` / `cargo test -p fxg-acp` / `pnpm --dir ui run check` / `pnpm --dir ui exec vitest run src/lib/sync/reducer.test.ts`

## 背景

ACP エージェントの 1 ターン内で思考 (`AgentThoughtChunk`) が回答
(`AgentMessageChunk`) より先にストリーミングされても、タイムライン上では
回答の下（後）に表示されていた。

原因は 2 つあった。

1. **永続化順序**: `StreamingAccumulator` は思考と回答を別々のバッファに
   蓄積し、完了イベントを「同じ種別の次のチャンク」またはターン終了時にしか
   出していなかった。さらに `finish_all` がメッセージ完了 → 思考完了の順で
   返していたため、`node_seq` が回答の後に思考へ割り当てられていた
   (`agent_message` → `agent_thought`)。
2. **ストリーミング表示順**: タイムライン構築時に `messageDeltas` をすべて
   並べてから `thoughtDeltas` を並べていたため、生成中の思考は常に回答の
   下に描画されていた。

## 変更内容

### 1. `fxg-acp`

- 思考 → 回答（または回答 → 思考）へ切り替わる時点で、それまでのストリームを
  即座に完了イベントとして確定するようにした。完了イベントは確定した瞬間の
  `node_seq` を消費するため、思考が回答より前の `node_seq` になる。
- `finish_all` は念のため思考 → 回答の順で返すようにした。
- 上記の順序を検証するユニットテストを追加。

### 2. `ui`

- `SessionTimeline` に `streamKeys` (最初のデルタが到着した順の
  `${kind}:${id}` 一覧) を追加し、`applyStreamDelta` で記録、
  `clearStreamBuffers` で削除するようにした。
- `buildTimelineItems` のストリーミング末尾は、メッセージ・思考・ツール・
  ターミナルをこの到着順でまとめて追加するようにした。
- 思考と回答が交互に到着するケースの Vitest を追加。

## 影響

- 新規ターンでは思考が回答より前に保存・表示される。
- 既存セッションの `node.db` に保存済みのイベント順序は変更しない
  (過去ターンの「回答 → 思考」はそのまま残る)。
