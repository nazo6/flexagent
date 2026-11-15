# エージェント利用上限 (5h / weekly / monthly)

- **日付**: 2026-11-15
- **対象パッケージ**: `fxg-protocol`, `fxg-agent`, `fxg-hub`, `fxg-node`, `ui`,
  `docs`
- **対象スクリプト**: `mise run fmt`, `mise run lint:errors`, `mise run types`,
  `cargo test --workspace`, `pnpm --dir ui run check`

セッションのエージェントが契約・アカウントに紐づく利用上限（OpenCode Go の
rolling 5h / weekly / monthly、Antigravity の 5h / weekly など）を取得し、
セッションヘッダーに表示できるようにした。

## 実装

- **プロトコル**: `AgentUsageLimits` / `UsageSource` / `UsageGroup` /
  `UsageWindow` / `UsageWindowKind` を追加。表示は **source → group →
  window** の3層とし、`5h` などの期間だけを単独表示しない。`used_fraction`
  は使用率に正規化し、未知の window は `Unknown` として保持する。
- **基礎トレイト**: `AgentDriver::usage_limits()` と
  `AgentExtension::usage_limits()` を追加（既定は `None`）。`AcpDriver` は
  extension に委譲し、`AgentCatalog::usage_limits()` が launch 解決と driver
  選択を行う。
- **API**: `GET /api/v1/nodes/:node_id/agents/:agent_id/usage` を追加。
  ハブは `NodeRpcRequest::AgentUsage` でノードに中継し、ローカルノードは
  in-process で実行する。未対応・未ログイン時は `usage: null`。
  `force=true` は driver 側のキャッシュを無視し、UI の明示的な Refresh が
  実際に再取得する。
- **OpenCode (Tier 3)**: ローカルサーバーの
  `/api/experimental/usage/limits` を優先し、データがない場合は
  `/api/credential` の Go キーで `https://opencode.ai/zen/go/v1/usage` を
  読む。キーはリクエスト内だけで使い、ログには残さない。
- **Antigravity (Tier 2)**: 新しい `antigravity` extension が
  `agy -p /usage --output-format json` を実行し、`command.data.groups` を
  `AgentUsageLimits` に変換する。プロファイル指定は ACP 起動と同じ値を
  引き継ぎ、60秒キャッシュする。
- **UI**: セッションヘッダーに利用上限チップを追加。チップは source と
  最も残量が少ない window を表示し、ポップオーバーで group / window /
  リセット時刻を表示する。取得できない場合は何も表示しない。ポップオーバーの
  Refresh は実行中に `Checking…`、成功時に `Updated just now`、失敗時に
  `Could not update` を表示し、結果が同じでも操作が伝わるようにした。

## テスト

- `fxg-agent`: OpenCode のローカル / 直接 API レスポンスのパース、
  Antigravity の JSON パース、extension への委譲、`agy` 実機の
  `#[ignore]` テスト。
- `fxg-protocol`: `mise run types` による TypeScript 型生成。
- UI: `svelte-check` と既存の Vitest。

## 手動確認 (2026-11-15)

- 実行中の OpenCode v2.2.7 で `GET /api/experimental/usage/limits` が
  `go` を返さない（Go API キーのみ）ことを確認し、`/api/credential` の
  キーで `/zen/go/v1/usage` が 200 と rolling / weekly / monthly を返す
  ことを確認した。
- インストール済み `agy` 1.5.8 で `/usage --output-format json` が
  `Gemini Models` / `Claude and GPT models` などのグループを返すことを
  確認した。
