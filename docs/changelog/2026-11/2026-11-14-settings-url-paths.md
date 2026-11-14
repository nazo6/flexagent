# Settings 画面の URL パス化（`?scope=&tab=` 廃止）

- **日付**: 2026-11-14
- **対象パッケージ**: `ui`, `docs`
- **対象スクリプト / 設定**: `mise run fmt`, `pnpm --dir ui run check`,
  `pnpm --dir ui run lint`, `pnpm --dir ui run test`, `pnpm --dir ui run build`

## 背景

これまでの Settings 画面（`ui/src/routes/settings/+page.svelte`）は `?scope=&tab=` のクエリパラメータを使用し、マウント時に1回だけ読み取ったローカル `$state` と `goto(url, { shallow: true, replace: true })` で状態を同期していたため、ブラウザの戻る・進むボタンや別タブへのリンク遷移で正しく画面が戻らない問題があった。

スコープ・対象ノード・タブを URL パスとして表現し、URL（`page.params` / `page.route.id`）を単一のソースとして表示を駆動する構成へ移行する。

## 変更

- **パスベースルーティングの導入（`ui/src/lib/settings-routes.ts`, `ui/src/routes/settings/`）**:
  - Hub: `/settings/hub/<tab>`（`general`, `process`, `tokens`, `provisioners`, `audit`）
  - Node（Direct Node 接続時も含む）: `/settings/nodes/<node_id>/<tab>`（`general`, `process`, `pairing`, `agents`, `audit`）
  - Browser: `/settings/browser/<tab>`（`appearance`, `notifications`）
  - Hub / Node 双方で `general` を先頭タブに変更し、各スコープの先頭タブ（Hub/Node は `general`、Browser は `appearance`）をデフォルトに統一。
  - 中間パス（`/settings`, `/settings/hub`, `/settings/nodes`, `/settings/nodes/<node_id>`, `/settings/browser`）のみ `replaceState: true` でデフォルト先へ補完リダイレクトし、オンラインノードが 0 件の `/settings/nodes` は空状態を表示。
  - スコープカード・タブ・検索結果ジャンプ・ノード選択ドロップダウンなどユーザー操作の遷移はすべて通常のブラウザ履歴に追加し、スコープカード切替時は遷移先スコープの先頭タブへ移動する（ノード選択ドロップダウンの切替時は現在のタブを維持）。
  - 未知のタブ・パスは 404 エラーページとし、利用不可なスコープやオフライン／未登録ノード（Node 直結時の `/settings/hub/*`、オフラインの `node_id`、Hub 経由時の `process`/`audit`）は画面内に案内を表示。
  - 旧 `?scope=&tab=` および `LEGACY_TABS` のフォールバックを削除。
- **共有コンポーネント整理と呼び出し元更新**:
  - `ui/src/lib/components/settings/` に `ClientTokenCard.svelte`・`ProcessSettingsPanel.svelte`・`NodeTokensPanel.svelte` を抽出し、Hub/Node 間の重複を排除。
  - `ui/src/lib/components/ui/tabs/tabs-trigger.svelte` から `tabsTriggerClass` をエクスポートし、`<a>` ベースのタブナビゲーションでスタイルを共有。
  - `ui/src/routes/+layout.svelte`（`handleProblemAction`）、`ui/src/lib/components/session/NewSessionChat.svelte`、`ui/src/lib/components/settings/NotificationsPanel.svelte`（`vapidSubjectHref`）の遷移先を新パスへ更新。
  - `ui/messages/en.json` にオフライン／未検出ノード・ローカル専用タブ用の案内メッセージを追加。
- **ユニットテスト追加**:
  - `ui/src/lib/settings-routes.test.ts` を追加。

## 検証

- `mise run fmt` / `mise run fmt:check`
- `pnpm --dir ui run check` (`svelte-check`, service-worker `tsc`)
- `pnpm --dir ui run lint` (`oxlint`)
- `pnpm --dir ui run test` (`vitest`)
- `pnpm --dir ui run build` (`vite build`)
