# 履歴を replay しない `session/load` をインポート失敗として扱う

- **日付**: 2026-11-14
- **対象パッケージ**: `fxg-acp`, `fxg-node`, `ui`
- **対象スクリプト**: `crates/fxg-acp/tests/external_sessions.rs`, `crates/fxg-acp/tests/fixtures/mock_agent.rs`, `crates/fxg-node/src/session_manager/tests.rs`

## 目的

外部エージェントのセッションを取り込む際、`session/load` が成功応答を返しても
履歴を 1 件も replay しないエージェントがある（例: agy が一覧に出す旧アプリの
会話を現行 CLI ストアから読み出せない場合）。従来は警告ログを出して空の履歴で
インポートを成功させていたため、ユーザーには「取り込めたが会話が空」としか
見えなかった。

## 決定

- `capture_history: true` のインポートで履歴を 1 件も受信しなかった場合、
  `HistoryCaptureUnavailable` を返す。ノードは既存のマッピングで
  `HISTORY_UNAVAILABLE` に変換し、作成済みのセッション行をロールバックする。
- 能力情報やタイトルなどのメタデータ更新だけでは履歴とみなさない。
  会話の復元を示す更新（user / agent / thought / tool call / plan など）を
  1 件以上要求する。
- ACP 仕様では replay は `session/load` 応答前に完了する。SDK の配送タイミング
  だけを吸収する短い猶予（500ms）を置き、それでも履歴が無ければ失敗とする。
- UI は `HISTORY_UNAVAILABLE` を受けたとき、トーストのアクションから
  `mirror_history: false` の取り込みを選べるようにする（暗黙の空インポートは
  行わない）。

## 制約

- 本当にメッセージが 1 件も無い外部セッションも `mirror_history: true` では
  失敗する。履歴なしの取り込みを選ぶ必要がある。
