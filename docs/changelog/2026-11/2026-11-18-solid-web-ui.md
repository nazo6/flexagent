# Web UI の Solid 2 移行

- **日付**: 2026-11-18
- **対象パッケージ**: `ui`、`fxg-ui-assets`、`fxg-api`
- **対象スクリプト / 設定**: `mise run build:ui` / `check:ui` / `test:ui` / `lint:ui`、`mise run types`

## 概要

Web UI を Svelte 5 / SvelteKit から **Solid 2 + Vite** の SPA へ移行し、`ui/` を
`rust-embed` で同梱する配信契約はそのまま維持した。型検査は `tsc --noEmit`
一本、ヘッドレス UI は Terracotta 2、ルーターは `@solidjs/router` 2 を使う。
移行の設計と経緯は
[`docs/proposals/2026-11-18-solid2-ui-migration.md`](../../proposals/2026-11-18-solid2-ui-migration.md)
を参照。

## 主な変更

- SvelteKit の `$app/*` / `adapter-static` / `svelte-check` を撤去し、
  `src/index.tsx`・`src/app/router.tsx`・`src/app/shells/AppShell.tsx` の
  構成にした。URL（ディープリンク・PWA ショートカット）は変更していない。
- `*.svelte.ts` の rune ストアを Solid のモジュールストアへ移植した。
  `ReactiveCell` / `ReactiveMap` / `ReactiveSet` が値の即時読み書きと
  signal による変更通知を分離し、WebSocket イベント適用の順序を保つ。
- UI プリミティブは Terracotta 2 と `@floating-ui/dom` の自前配置で再実装した。
  Dialog / AlertDialog は panel を `<body>` へ portal し、サイドバーのリンク内に
  置かれたメニューから開いても周囲のリンクへクリックが漏れない。
- `mode-watcher` / `svelte-sonner` / `vaul-svelte` / `@lucide/svelte` を
  `lib/mode.ts`・Terracotta toast・自前 Drawer・`unplugin-icons` に置き換えた。
- PWA は Vite のビルド後プラグインが `sw.js` を生成し、ビルド資産の一覧を
  注入する。`index.html` と `sw.js` は `no-cache` で配信し、それ以外の
  ハッシュ付き資産は immutable とする。
- `tsc` / `Vitest` の前に Paraglide の生成を必須にした
  (`pnpm --dir ui run check` / `test` が `i18n:compile` を先に実行する)。

## 検証

- `pnpm --dir ui run check` / `test`: 成功（25 ファイル / 212 テスト）
- `pnpm --dir ui run build`: 成功（`ui/build` 約 16 MB、main chunk 約 1.5 MB）
- ヘッドレス Chromium で `/`、`/projects`、`/search`、設定、セッション画面、
  ドック（Files / Changes / Terminal）、404、ライト / ダーク / 3 パレット、
  モバイル幅を確認。コンソールエラーなし
- 実ノードでエージェント応答、Web ターミナル接続、Files ツリーと
  Markdown プレビューを確認
