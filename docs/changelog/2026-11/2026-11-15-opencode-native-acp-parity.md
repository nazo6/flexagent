# OpenCode native driver の ACP パリティ

- **日付**: 2026-11-15
- **対象パッケージ**: `fxg-agent`, `fxg-cli`, `docs`
- **対象スクリプト**: `mise run fmt`, `cargo test -p fxg-agent`,
  `cargo test -p fxg-cli`, `cargo clippy -p fxg-agent -p fxg-cli --all-targets`

OpenCode の ACP 経路と native 経路の差分のうち、native 側で埋められるものを
実装した。

## 実装

- **todo / plan**: `todowrite` の入力 (`todos`) を `PlanUpdate` に変換する。
  JSON 文字列として渡された入力も受け付ける。空の todo リストはプランを
  クリアする。ACP の `SessionUpdate::Plan` と同じく、子セッションの
  `subagent_id` も保持する。
- **打ち切り理由**: `session.step.ended` の `finish` と
  `session.step.failed` / `session.execution.failed` の `error.type` を
  保持し、`finish = "length"` を `MaxTokens`、`content-filter` を `Refusal`
  として `TurnEnded` を発行する。`provider.content-filter` はエラーではなく
  refusal として idle に戻す。
- **ネイティブ認証**: `AgentProvider::auth()` を追加し、native provider は
  terminal の login / logout コマンドを宣言できる。`opencode` は
  `opencode auth login` / `opencode auth logout` を宣言する。
  `fxg agents login/logout opencode` は ACP `initialize` ではなく、この
  コマンドを実行する。custom 定義は従来どおり ACP の認証方法を使う。
- **認証エラーの案内**: `provider.auth` のエラーを、`opencode auth login` /
  `fxg agents login opencode` を案内するメッセージに変換する。
- **コマンドのライブ更新**: `session.execution.succeeded` でも
  capabilities を再取得する。前回と同じ payload は再送しない。
- **close 相当の停止**: `shutdown` は実行中のターンを `interrupt` してから
  購読を解除する。ACP の `session/close` と同じく、保存済みセッションは
  残す。

## 制約

- native は OpenCode v2 のみを対象とする。v1 は `opencode-acp` が残る間だけ
  利用できる。
- ACP の `effort` は native では `model` の `#variant` に統合したままとする。
  モデル選択で同じ値を選べるため、別の config option は追加しない。
- `additionalDirectories` と session-scoped MCP servers は fxg の
  `StartSessionRequest` に存在しないため、ACP 経路でも渡していない。
  必要になった時点で両方に追加する。
