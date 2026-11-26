# コンポーザーの添付 UI 共通化

- **日付**: 2026-11-26
- **対象パッケージ**: `ui`, `docs`
- **対象スクリプト**: `mise run fmt`, `pnpm --dir ui run check`

---

## 概要

新規セッション画面 (`NewSessionChat.svelte`) とセッション詳細画面 (`Composer.svelte`)
は、添付チップの一覧と画像添付ボタンのマークアップをそれぞれ持っていた。見た目や
削除・再選択の挙動が片方だけ変わらないよう、共有コンポーネントへ寄せた。

## 変更

### `ui`

- `ComposerAttachments.svelte` を追加し、添付チップ（画像サムネイル / ファイル名 /
  削除）を共通化した。
- `ComposerAttachImageButton.svelte` を追加し、非表示のファイル入力とペーパークリップ
  ボタン、選択後の input リセットを共通化した。
- 両画面はこれらのコンポーネントを使うように変更し、重複していた `fileInput` 状態と
  `onFileInputChange` ハンドラを削除した。

## 検証

- `pnpm --dir ui run check`
- `pnpm --dir ui run test` (235 tests)
- 実ブラウザ (390×844): セッション詳細と新規セッションの両方で画像を添付し、
  チップが表示され削除ボタンが出ることを確認。
