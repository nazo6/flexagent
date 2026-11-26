# PWA のセーフエリア対応

- **日付**: 2026-11-26
- **対象パッケージ**: `ui`, `docs`
- **対象スクリプト**: `mise run fmt`, `pnpm --dir ui run check`

---

## 概要

`app.html` は `viewport-fit=cover` を指定していたが、`env(safe-area-inset-*)` を
どこにも使っていなかった。ホーム画面に追加した PWA では、ステータスバー・ノッチ・
ホームインジケータがヘッダー、コンポーザー、下部ナビゲーションに重なる可能性があった。

## 変更

### `ui`

- `app.css` に `--safe-top` / `--safe-right` / `--safe-bottom` / `--safe-left`
  （`env(safe-area-inset-*, 0px)`）を追加し、`.safe-area-shell` と
  `.safe-area-portal` を定義した。
- アプリシェルのルート（接続待ち・エラー・トークン入力・通常画面）に
  `.safe-area-shell` を適用した。通常のブラウザタブやノッチのないウィンドウでは
  すべて 0 なのでレイアウトは変わらない。
- body へ portal されるモバイルドロワー（`Sheet.Content`）に `.safe-area-portal` を
  適用した。
- トーストの上端オフセットに `--safe-top` を足した。

## 検証

- `pnpm --dir ui run check`
- 実ブラウザ (390×844 / touch): `--safe-top: 47px` / `--safe-bottom: 34px` を
  模擬注入し、モバイルヘッダーが 47px 下がること、コンポーザーが下端 34px の内側に
  収まること、ドロワーがセーフエリア内に収まることを確認。
