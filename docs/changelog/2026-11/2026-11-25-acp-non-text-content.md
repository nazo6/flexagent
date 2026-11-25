# ACP の非 Text コンテンツ（画像・音声・リソース）を保持する

- **日付**: 2026-11-25
- **対象パッケージ**: `fxg-protocol`, `fxg-agent`, `fxg-db`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run fmt`, `cargo test -p fxg-protocol -p fxg-agent -p fxg-db`,
  `pnpm --dir ui run test`, `pnpm --dir ui run check`

---

## 概要

ACP の `AgentMessageChunk` / `AgentThoughtChunk` は Text 以外にも
`Image` / `Audio` / `ResourceLink` / `Resource` を運べるが、fxg は Text 以外を
黙って破棄していた。エージェントが返した画像やリソースが Web UI に表示されず、
`session/load` で復元したユーザーメッセージの添付も失われていた。

## 変更

### `fxg-protocol`

- `AgentMessage` / `AgentThought` に `attachments: Vec<AttachmentMeta>` を追加した
  (`serde(default)`。既存イベントは空配列)。ユーザーメッセージの
  `UserMessage.attachments` と同じ表現を使う。イベントログは JSON なので
  DB マイグレーションは不要。

### `fxg-agent`

- ストリームの蓄積を `(id, text)` から `PendingStream { id, text, attachments }`
  へ変更し、非 Text ブロックを同じストリームに蓄積する。テキストが無い
  画像だけの応答でも、ターン終了時に空テキスト + 添付として完成イベントを出す。
- マッピング:
  - `Image` / `Audio` / `Resource(blob)` は Base64 をインライン添付にする。
  - `ResourceLink` はファイル名・MIME・`file:` URI のローカルパスを保持する。
  - `Resource(text)` は本文テキストとして会話に含める。
- 10 MiB を超えるインライン画像などはイベントを肥大化させないため、
  メタデータ（ファイル名・MIME・サイズ）だけを保持する。
- `session/load` の Replay で復元するユーザーメッセージの添付も同様に保持する。

### `fxg-db`

- `AgentMessage` / `AgentThought` の添付ファイル名を FTS5 の検索対象に含める。

### `ui`

- タイムラインのエージェントメッセージ / 思考項目が `attachments` を保持し、
  画像はインライン表示、それ以外はファイル名のチップとして表示する。
  ユーザーメッセージと共通の `MessageAttachments` コンポーネントに集約した。

## 制約

- 画像は完成イベントで初めて表示される（`LiveStreamDelta` はテキスト専用の
  ため、ストリーミング中は出ない）。
- 10 MiB 超のインラインデータは表示せず、メタデータのみ残す。

## 検証

- `cargo test -p fxg-agent --lib`（画像のみの応答、テキストとの併存、
  `ResourceLink` のローカルパス）
- `pnpm --dir ui run test`（reducer が添付を保持する）
- `mise run types` (`ui/src/lib/generated/bindings.ts` 同期)
