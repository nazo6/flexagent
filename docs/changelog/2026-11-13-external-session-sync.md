# 外部エージェントセッションの同期（発見と取り込み）

- **日付**: 2026-11-13
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-db`, `fxg-node`, `fxg-hub`, `fxg-cli`, `ui`
- **対象スクリプト**: `crates/fxg-acp/tests/external_sessions.rs`, `crates/fxg-node/src/session_manager/tests.rs`, `ui/src/lib/components/session/ExternalSessionsDialog.svelte`

## 目的

既存の ACP エージェントが持っているセッションを fxg の一覧から見つけ、
必要なものだけ取り込めるようにする。エージェント定義の自動登録は行わない
（ACP に検出プロトコルが無く、`PATH` の有無だけでは ACP 対応を判定できない）。

## 決定

- **発見は読み取り専用**: `session/list` を短命プロセスで呼び、`session/new` は
  作らない。エージェントごとのタイムアウトと全体予算、短時間キャッシュを持つ。
- **取り込みは開いた時だけ**: 外部一覧に `imported_session_id` を付け、未取り込み
  のみ `POST /api/v1/sessions/import` を呼ぶ。自動取り込みはしない。
- **履歴は `session/load` の replay をミラー**: `capture_history` を追加し、
  import モードで replay のユーザーメッセージも記録する。live ターンの
  「manager がユーザーメッセージを記録済み」という前提は変えない。
- **失敗時はロールバック**: 取り込み起動に失敗したセッション行は
  `SessionDeleted` で削除し、重複判定に残さない。
- **`HISTORY_UNAVAILABLE`**: `loadSession` 非対応で履歴ミラーを要求された場合の
  エラーコード。UI は履歴なしの取り込みを提案する。

## API

- `GET /api/v1/sessions/external?node_id=&agent=&force=`（ノードはローカル、
  ハブは `node_id` 必須で RPC 中継）。
- `POST /api/v1/sessions/import` に `mirror_history`（既定 `false`）を追加。
- `NodeRpcRequest::ListExternalSessions` / `NodeRpcResponse::ExternalSessions`。
- CLI: `fxg session external [--agent <id>] [--force] [--json]`、
  `fxg session import <id> --agent <id> --mirror-history`。

## 制約・既知の制限

- `session/list` 非対応のエージェントは発見できない。
- replay のタイムスタンプは取得できないため、取り込みイベントの時刻は
  取り込み時刻になる。
- 取り込み後に外部 CLI 側で会話が進んだ場合の差分再同期は行わない
  （ACP に差分 API が無い）。
- replay の画像・リソース添付はテキストのみに落とす。
