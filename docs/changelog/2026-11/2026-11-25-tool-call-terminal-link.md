# ツール呼び出しと ACP 端末カードのリンク

- **日付**: 2026-11-25
- **対象パッケージ**: `fxg-protocol`, `fxg-agent`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run fmt`, `cargo test -p fxg-protocol -p fxg-agent`,
  `pnpm --dir ui run test`, `pnpm --dir ui run check`

---

## 概要

ACP の `ToolCallContent::Terminal` は、ツール呼び出しが埋め込む端末
(`terminal/create` で開始したコマンド) を `terminal_id` で参照する。fxg は
この情報を破棄していたため、ツール行と端末カードが別々に表示され、どの
ツールがどの出力を生んだのか辿れなかった。

## 変更

### `fxg-protocol`

- `UnifiedEventPayload::ToolCall` に `terminal_id: Option<String>` を追加した
  (`serde(default)`。既存イベントは `None` = 端末参照なし)。
  イベントログは JSON ペイロードなので DB マイグレーションは不要。

### `fxg-agent`

- `tool_call_contents` が `ToolCallContent::Terminal` から `terminal_id` を
  取り出し、`ToolCall` / `ToolCallUpdate` の両方でイベントへ載せる。
- 端末 ID は後続の `ToolCallUpdate` でも保持される (差分更新で消えない)。

### `ui`

- タイムラインのツール項目が `terminalId` を保持し、ツール行に
  「Terminal output」リンクを表示する。リンクは端末カードのアンカー
  (`#terminal-<id>`) へ移動する。
- ツールから参照されている端末カードは、コマンド終了後も作業ブロックに
  折りたたまず表示する。折りたたむとリンク先が DOM に存在しなくなるため。

## 検証

- `cargo test -p fxg-protocol -p fxg-agent`
- `pnpm --dir ui run test` (reducer の `terminalId` 保持、終了済み端末を
  折りたたまないグループ化)
- `mise run types` (`ui/src/lib/generated/bindings.ts` 同期)
