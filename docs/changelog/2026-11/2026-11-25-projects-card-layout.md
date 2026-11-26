# プロジェクト一覧のカード化

- **日付**: 2026-11-25
- **対象パッケージ**: `ui`
- **対象スクリプト**: `mise run fmt`, `pnpm --dir ui run check`, `pnpm --dir ui run lint`, `pnpm --dir ui run test`

---

## 概要

`/projects` は 1px の下線だけで各プロジェクトを並べていた。区切り線は
背景とのコントラストが約 1.3:1 しかなく、プロジェクト名以外のメタ情報
(パス・ブランチ・最終使用・ノード) が 11px の muted で並ぶため、どこまでが
1 プロジェクトなのか、どの行がフォルダでどの行が worktree なのかを
視線で追いにくかった。色も `path missing` の赤だけに現れ、識別子の
種類 (Git remote / User key / Local only) は無彩色のバッジだった。

各プロジェクトを面 (カード) の単位にし、フォルダと worktree をラベル付きの
行として並べる。色は「共有されない」「パスが無い」という判断に必要な
2 種類だけに絞る。

## 変更

### `ui`

- `ProjectCard` を `bg-card` + `border-line-strong` + `rounded-lg` のカードに
  した。ページ側は区切りの `border-t` をやめ、カード間の `gap` で並べる。
- 見出しは 14px semibold、メタ情報は 12px に統一した (11px は
  `.desk-label` の大文字ラベルだけ)。パスは 1 行で `truncate` し、
  折り返しで行の高さが崩れないようにした。狭い画面では `basis` を
  確保してパスが次の行へ回る。
- identity バッジを意味のある表現にした。`Git remote` / `User key` は
  outline、`Local only` は `warn-soft` + 警告アイコン (共有されないことの
  注意) とし、説明は tooltip に移した。
- フォルダ行・worktree 行は `bg-background` の面 + hover のボーダーで
  区切り、欠損パスの行だけ `danger-soft` の面と `Missing` バッジを出す。
- ノード名・`main`・`worktree` は secondary バッジにして、本文 (パス・
  ブランチ) より手前に出ないようにした。
- Git プロジェクトは URL を、それ以外は project ID を 1 行だけ表示する
  (Git では ID が URL と重複するため)。
- i18n: `projects_folders` / `projects_local_only_hint` を追加した。

## 検証

- `pnpm --dir ui run check`
- `pnpm --dir ui run lint`
- `pnpm --dir ui run test`
- 手動: dark テーマの実データで、390px (モバイル) と 1120px (デスクトップ)
  相当の幅でカード・行の折り返しと worktree 読み込み後の表示を確認した。
