# OpenCode を native driver に一本化する

- **日付**: 2026-11-25
- **対象パッケージ**: `fxg-agent`, `docs`
- **対象スクリプト**: `cargo test -p fxg-agent`,
  `cargo clippy -p fxg-agent --all-targets`, `mise run fmt`

OpenCode の ACP 経路 (`opencode acp` + `OpenCodeExtension`) を削除し、
native driver (`OpenCodeNativeDriver` / `opencode serve`) に一本化した。

## 変更

- ビルトイン `opencode-acp` と `OpenCodeExtension` を削除した。
  OpenCode 用の extension を削除しただけで、汎用の `AgentExtension` 機構と
  `antigravity`（利用上限）は残す。
- `CustomAgentConfig.extension = "opencode"` を参照する既存の custom 定義は
  未知の extension として起動時に拒否される。OpenCode は builtin `opencode`
  （native）を使う。
- 外部セッションの発見と取り込みは native の `list_sessions` と
  `capture_history` 経路で継続する。`mirror_history` は `/api/session/{id}/message`
  からユーザー / アシスタント / reasoning / ツール呼び出しを `Import` として
  記録する。
- native driver の子セッション処理を ACP extension と同等にした。
  - `session.execution.succeeded` と `session.idle` の二重通知で
    `SubagentUpdated(Completed)` を重複して出さない。
  - `session.renamed` / `session.title.updated` を子の状態へ反映し、後続の
    status 更新で古いタイトルに戻らないようにした。
  - 子の権限要求・ツール呼び出しが `subagent_id` を持つことを契約テストで
    固定した。
- `docs/03-protocol-and-api.md` と `docs/04-agent-drivers-and-windows.md` の
  サブエージェント正規化の説明を native の経路へ更新した。

## 制約

- native driver は OpenCode v2 の HTTP API のみを対象とする。`opencode acp`
  でしか動かない v1 はサポートしない。
- `opencode-acp` で作成済みの fxg セッションは、native のセッションストアを
  持たないため再開できない。移行機能は提供しない（初回デプロイ前のため）。
