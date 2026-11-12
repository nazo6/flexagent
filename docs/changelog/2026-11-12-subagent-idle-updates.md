# 2026-11-12 サブエージェント完了後の結果が次のメッセージまで表示されない問題の修正

- **日付**: 2026-11-12
- **対象パッケージ**: `fxg-acp`
- **対象スクリプト**: `mise run fmt`, `cargo test -p fxg-acp`,
  `cargo clippy -p fxg-acp --all-targets`

## 背景

agy (Antigravity CLI) の `/teamwork` は、親ターンを `end_turn` で終了した後に
サブエージェントの報告を同じ root セッションへ送る。agy の ACP サーバは
アイドル中に会話更新を転送せず、次の `session/prompt` を受けた時点で溜まった
更新をまとめて送る（実機ログ: `Stream goroutine exited` →
次のプロンプトで `Starting conversation update stream`）。

fxg の `serve_session` は `session.read_update()` をターンループ内でしか
呼んでおらず、プロンプト開始時に accumulator を `reset()` していた。そのため:

- ターン外に届いた root 宛の更新は次のプロンプトまで読まれない。
- 読まれた報告は、`messageId` を付けないエージェント（agy）では新しい回答と
  同じ `acp-message` バッファに連結され、1つの `agent_message` になる。

opencode の `task` は親ターン内で子が完了するため発生しない。

## 変更内容

- **アイドル中の読み取り**: `serve_session` の外側ループに
  `session.read_update()` を追加し、ターン外の root 宛 `session/update` を
  即座に正規化する。`PromptResponse` が無いため、最後の更新から
  `IDLE_FLUSH_DELAY` (500ms) 静かになった時点で `finish_all()` して
  完了イベントを確定する。
- **プロンプト開始時の確定**: `accumulator.reset()` の前に、すでに配送済みの
  アイドル中の更新を drain して `finish_all()` を行い、内容を破棄しない。
- **ターン境界**: `user_message_chunk` を境界として、それ以前の root の
  ストリームを確定する。agy が次のプロンプトで流す保留報告が、新しい回答と
  1つの `agent_message` に連結されない。
- 子 / vendor 通知（ingress）もアイドル中は最終更新時刻を更新し、子が
  動いている間は確定を遅らせる。

## 検証

- `crates/fxg-acp/tests/idle_updates.rs` と mock ACP エージェントの
  `--mode late-updates` を追加。
  - プロンプト応答後に届いた報告が、2回目のプロンプト無しで確定・配信される。
  - 次のプロンプトの先頭で流れる保留報告が、新しい回答と別の
    `agent_message` になる。
- `cargo test -p fxg-acp`, `cargo clippy -p fxg-acp --all-targets` が通る。

## 制約

agy はアイドル中に通知自体を送らないため、「チーム完了直後」に表示することは
クライアント側だけではできない。本修正は「次にプロンプトを送った時点で、
前のターンの結果として正しく・即座に表示する」までを保証する。
