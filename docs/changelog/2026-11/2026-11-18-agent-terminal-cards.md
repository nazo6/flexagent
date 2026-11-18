# エージェント端末カードのコマンド表示・個別停止・ターン終了スナップショット

- **日付**: 2026-11-18
- **対象パッケージ**: `fxg-protocol`, `fxg-agent`, `fxg-node`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run fmt`, `mise run lint:errors`,
  `cargo test --workspace --no-fail-fast`, `pnpm --dir ui run test`,
  `pnpm --dir ui run check`

---

## 概要

ACP エージェントが `terminal/*` で開始したコマンドは、ターンを跨いで動き続け
られる。従来は次の 3 点が扱いにくかった。

1. 実行中の端末カードの見出しがコマンド名ではなく terminal ID（UUID）だった。
   コマンド名は終了時のイベントにしか含まれていなかった。
2. 動き続けるバックグラウンドコマンドを止める手段が、ターンの中断（協調的で
   コマンドは残りうる）か、セッション全体の終了しかなかった。
3. 実行中の出力が永続化されず、リロードするとカードごと消えていた（終了時の
   イベントだけが保存されていた）。

## 変更

### 端末イベントのライフサイクル (`fxg-protocol`)

- `TerminalOutput` に `running: bool` を追加した。`running = true` は実行中、
  `false` は終了イベント。既存のイベントは終了イベントなので `serde(default)`
  で `false` になる。
- 1 つのターミナルは「開始イベント（空の出力）→ ターン終了時のスナップショット
  → 終了イベント」の複数イベントで記録し、各イベントは**前回以降に増えた
  バイト列だけ**を持つ。UI はそのまま追記でき、スナップショットを挟んでも
  出力が重複しない。

### 開始イベントとターン終了スナップショット (`fxg-agent`, `fxg-node`)

- `terminal/create` の直後に開始イベントを記録する。これで実行中もコマンド名が
  表示され、リロード後も「何が動いているか」が残る。
- `ActiveSessionHandle::snapshot_terminals` を追加し、ノードがターン終了
  （`StatusChanged` が非 busy）を観測した時点で、実行中のターミナルへ
  「未保存の出力を公開せよ」と依頼する。出力収集タスクが唯一の書き手なので、
  ライブ配信との順序が入れ替わって重複することはない。
- 出力が増えていないターンではイベントを追加しない。

### コマンド単体の停止 (`fxg-protocol`, `fxg-agent`, `fxg-node`, `ui`)

- `SessionControlAction::KillTerminal { terminal_id }` を追加した。
  `ActiveSessionHandle::kill_terminal` が ACP ターミナルの PTY を終了し、通常の
  終了と同じ `TerminalOutput`（exit code 付き）が流れる。
- 端末カードは `running` の間だけ停止ボタンを表示する。ターンやセッションは
  止めないため、バックグラウンドの dev server などをそのまま終了できる。
  端末を所有しないドライバ（OpenCode ネイティブなど）は端末カード自体を
  作らないため、この操作の対象にならない。
- 実行中の端末カードは「作業ブロック」に折りたたまず表示し続ける。ターン終了後
  も止められるようにするためで、終了すると折りたたみ対象に戻る。
- セッション終了時は端末レジストリを破棄し、ウォームプールで再利用する
  プロセスに前のセッションの出力バッファを残さない。

## 検証

- `cargo test --workspace --no-fail-fast`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `mise run lint:errors`
- `pnpm --dir ui run test`（開始イベントのコマンド表示、スナップショットの
  重複防止、終了イベントの追記）
- `pnpm --dir ui run check`
- `mise run fmt:check`
