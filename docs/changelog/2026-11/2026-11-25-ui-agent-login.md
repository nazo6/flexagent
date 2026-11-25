# Web UI から agent 型の ACP 認証を実行する

- **日付**: 2026-11-25
- **対象パッケージ**: `fxg-protocol`, `fxg-api`, `fxg-hub`, `fxg-node`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run fmt`, `cargo test -p fxg-node -p fxg-api -p fxg-hub`,
  `pnpm --dir ui run test`, `pnpm --dir ui run check`

---

## 概要

`AUTH_REQUIRED` になったとき、Web UI は設定画面へ移動するだけで、実際の
ログインは `fxg agents login <agent>` をノードの端末で実行する必要があった。
スマホ / PWA やリモートノードでは端末を開けないため、agent 型
(`authenticate` を送るだけ) の認証方法を Web UI から実行できるようにした。
terminal 型は対話シェルが必要なので従来どおり CLI の導線を表示する。

## 変更

### `fxg-protocol`

- `AgentAuthMethodsResponse` / `AgentLoginRequest` / `AgentLoginResponse` を追加。
- `NodeRpcRequest::AgentAuthMethods` / `AgentLogin` と対応する
  `NodeRpcResponse` を追加（ハブからノードへ中継）。

### `fxg-api`

- `ClientApiBackend::agent_auth_methods` / `agent_login` を追加。
- `GET /api/v1/nodes/:node_id/agents/:agent_id/auth-methods`
- `POST /api/v1/nodes/:node_id/agents/:agent_id/login`

### `fxg-node`

- `agent_auth` モジュールに、起動スペック解決・ACP `initialize` からの認証方法
  取得・agent 型 `authenticate` の実行を集約した。ローカル Client API と
  Node RPC の両方が同じ処理を使う。
- terminal 型を `/login` に渡した場合は `fxg agents login ...` を案内する
  エラーを返す。認証成功は `agent_manage` の監査ログに
  `{ action: "login", agent_id, method_id }` として残す。

### `fxg-hub`

- 上記 2 エンドポイントを `NodeRpcRequest` で対象ノードへ中継し、成功時に
  `hub.db` の監査ログへ記録する。

### `ui`

- エージェント一覧の各行に「Sign in」ボタンを追加。ダイアログを開いた時点で
  認証方法を取得し、agent 型はボタンで実行、terminal 型は
  `fxg agents login <id> --method <id>` を表示する。
- `AUTH_REQUIRED` の復旧アクションは
  `/settings/nodes/<node>/agents?login=<agent>` へ遷移し、該当エージェントの
  ダイアログを自動で開く。

## 制約

- agent 型の `authenticate` が完了するまで HTTP リクエストは待機する
  （ノード RPC のタイムアウトは 120 秒）。ブラウザ側の OAuth 完了を待つ
  エージェントでは、その完了までダイアログにスピナーが出る。
- terminal 型の Web UI 実行（PTY の埋め込み）は未対応。
- ログアウトは Web UI からは実行しない（`fxg agents logout` のまま）。

## 検証

- `cargo test -p fxg-node --lib agent_auth`
  （組み込みの terminal 方法の取得と、UI ログインでの拒否）
- `cargo check --workspace --all-targets`
- `pnpm --dir ui run check`
- `mise run types` (`ui/src/lib/generated/bindings.ts` 同期)
