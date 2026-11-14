# 自動承認時の承認カード表示を修正

- **日付**: 2026-11-14
- **対象パッケージ**: `fxg-node`, `ui`
- **対象スクリプト / 設定**: `cargo test -p fxg-node`, `pnpm --dir ui run check`,
  `pnpm --dir ui run lint`, `pnpm --dir ui run test`
- **関連ドキュメント**:
  [`docs/04-agent-drivers-and-windows.md`](../../04-agent-drivers-and-windows.md) §4.2

## 背景

`auto_allow_all` で自動承認したときも、チャットに `Permission required` の
カードが残り、承認ボタンが押せる状態になっていた。

原因はノードのイベント記録順で、`PermissionRequest` を永続化する前に
`PermissionResolved` を記録していたこと。`permission_requests` の投影は
解決イベントの時点で対象行がなく更新 0 件となり、その後に挿入された
リクエストが `pending` のまま残っていた。クライアントのタイムラインも
解決イベントを先に受けてリクエストを後から追加するため、未解決のカードとして
表示されていた。

## 変更

- **ノード**: `pump_events` は `PermissionRequest` を記録した後に自動承認を
  実行するよう順序を修正した。自動承認の処理は
  `auto_respond_permission` に分離し、応答に失敗した場合はリクエストを
  `pending` に残して手動承認できる状態を維持する。
- **UI**: `PermissionCard` は解決済みのとき `Permission required` ではなく
  状態に応じて `Auto-approved` / `Permission granted` / `Permission denied` を
  表示する。選択されたオプションは ACP の option ID ではなく表示名で示す。
- **UI (既存履歴)**: 修正前のイベントログでは解決イベントが先に並ぶため、
  `buildTimelineItems` は先行した `PermissionResolved` を保持し、後から届いた
  リクエストに適用する。既存セッションでも承認ボタンが残らない。
- **テスト**: 自動承認後に `permission_requests` が `approved` になり、
  `resolved_by` が `auto` になることを `fxg-node` のテストで確認する。
  解決イベントが先に来るケースのタイムライン組み立ても UI テストで確認する。

## 注意（変更しない不変条件）

- `auto_allow_all` はエージェントが提示した `allow_*` オプションのうち
  先頭のものを自動選択する。
- 承認の解決は従来通り `PermissionResolved { resolved_by: "auto" }` として
  イベントログと `permission_requests` に残す。
