# OpenCode2 bridge のスラッシュコマンド対応 (一覧取得 + `/command` 振り分け)

- **日付**: 2026-10-02
- **対象パッケージ**: `fxg-acp`
- **対象スクリプト / 設定**: `mise run fmt`, `cargo test -p fxg-acp --lib`,
  `cargo clippy -p fxg-acp --all-targets`

## 概要

`opencode serve` ブリッジ (既定モード) ではスラッシュコマンドが UI
に 1 件も表示されず、`/name` と入力しても通常のプロンプトとして LLM
へ渡っていました。原因は 2 つあり、どちらも bridge ドライバ側の未実装でした。

1. `fetch_capabilities` が `/api/agent` と `/api/model` しか取得せず、
   `available_commands` を常に空で送出していた
   (`crates/fxg-acp/src/opencode2.rs`)。
2. `send_prompt` が常に `/api/session/{id}/prompt` を呼んでいた。opencode の
   `/prompt` はスラッシュを解釈せず、本文がそのまま LLM へ渡る。

opencode v2.0.21 の実機観測で確定した仕様に合わせて両方を実装しました。

| エンドポイント                   | 挙動                                                                                  |
| :------------------------------- | :------------------------------------------------------------------------------------ |
| `GET /api/command`               | `{ data: [{ name, description? }] }`。組み込み (`init` / `review`) + プロジェクト定義 |
| `POST /api/session/{id}/command` | `{ name, text }` でコマンド実行。204 No Content (inboxID を返さない)                  |
| `POST /api/session/{id}/prompt`  | `/name ...` を解釈せず、素のテキストとして LLM へ渡す                                 |

## 実装

| ファイル                               | 内容                                                                                                                                          |
| :------------------------------------- | :-------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/fxg-acp/src/opencode2.rs`      | `fetch_capabilities` に `GET /api/command` を追加し `available_commands` を同期                                                               |
| 〃                                     | `SessionState` に `commands` (既知のコマンド名) と `pending_commands` (自己送信予約カウンタ) を追加                                           |
| 〃                                     | `send_prompt` で先頭トークンが既知の `/name` なら `POST /api/session/{id}/command` へ振り分け (引数は `text`、改行は保持)                     |
| 〃                                     | `split_command` を切り出し、未知の名前・コマンド形式でない入力は通常プロンプトへフォールバック                                                |
| 〃                                     | `commands_from_api` で `/api/command` の応答を `CommandInfo` へ変換 (`name` 無し要素は無視)                                                   |
| 〃                                     | SSE `session.inbox.enqueued` で `pending_commands` を消費し、コマンドターンの inboxID を `prompt_ids` に記録 (二重記録防止 + Revert 対応付け) |
| `docs/04-agent-drivers-and-windows.md` | モードA にスラッシュコマンドの仕様 (1 プロンプト 1 コマンド、TUI 専用コマンド非対応) を追記                                                   |

## 設計判断

- **1 プロンプト 1 コマンド**: opencode の ACP (`detectSlashCommand`) と純正TUI
  はどちらも先頭のコマンドだけを実行し、残りを引数として扱う。bridge
  もこれに合わせ、`/a /b` は「`a` の引数が `/b`」となる
  (複数実行の独自拡張はしない)。
- **既知のコマンドのみ実行**: 一覧に無い名前は `/prompt` へフォールバックする。
  `/command` は未知の名前で 404 `CommandNotFoundError` を返すため、ACP
  実装と同じ「通常のプロンプトとして送る」挙動に揃える。一覧取得に失敗した場合は
  空リストとなり、従来どおり全入力を通常プロンプトとして送る。
- **コマンド一覧は安定するまで待つ**: `/api/command` は起動直後に空を返し、
  さらに組み込みコマンド → プロジェクト定義コマンドの順に追加される (実測 +100ms
  程度)。`wait_for_capabilities` は「2 回連続で同一のコマンド一覧」かつ
  モード/モデルのカタログが揃うまで待ち、タイムアウト時は最新の部分的な内容を返す。
- **inboxID の記録は SSE 経由**: `/command` は 204 で inboxID
  を返さないため、`/prompt` のように応答から自己送信分を判別できない。送信前に
  `pending_commands` を立て、SSE の `session.inbox.enqueued`
  で消費して ID を記録する (先に届く SSE と競合しない)。
  これにより `UserMessage` の二重記録と、Revert のターン対応付けのずれを防ぐ。

## 制約

- `/undo`・`/redo`・`/share`・`/help`・`/compact` など opencode の TUI
  がローカル処理するコマンドは `/api/command` に含まれないため、bridge
  からは実行できない (`opencode_mode = "acp"` では ACP 側が `/compact` のみ
  `session/summarize` に振り分ける)。
- 引数の展開 (`$1`…`$n` / `$ARGUMENTS` / プレースホルダ無しの場合の本文追記) は
  opencode 側の実装に完全に委ねる。

## 検証

- `cargo test -p fxg-acp --lib` (ユニットテスト: `split_command` /
  `commands_from_api` / SSE の自己送信判別)。
- `cargo test -p fxg-acp --lib -- --ignored runs_slash_command_against_real_opencode2`
  (実 opencode v2.0.21: カスタムコマンドの一覧取得、`/ping hello world` が
  `$ARGUMENTS` 展開済みのユーザーメッセージとして opencode 側に残ること、
  SSE から二重記録されないこと)。
- `cargo test -p fxg-acp --lib -- --ignored serves_and_streams_against_real_opencode2`
  (起動時 capabilities に組み込みコマンドが含まれること)。
