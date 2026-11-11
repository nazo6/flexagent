# UI の入力欄・エージェント追加ダイアログ・選択状態を調整

- **日付**: 2026-11-11
- **状態**: 実装済み
- **対象パッケージ**: `ui`
- **対象スクリプト**: `mise run fmt`, `pnpm --dir ui run check`,
  `pnpm --dir ui run lint`, `pnpm --dir ui run test`

---

## 背景

Desk デザインシステム適用後の UI で、次の使いにくさが残っていた。

- 等幅フォントの入力欄 (エージェント ID / コマンド / 環境変数など) で、
  プレースホルダまで等幅になり本文と混ざって見える。
- カスタムエージェント追加ダイアログの Environment variables ラベルが、
  インラインの `code` と折り返しで崩れている。
- ACP Registry のカタログ取得と import が「Update catalog」ボタンと
  別ダイアログに分散している。
- サイドバーの選択中セッション / ナビゲーションの背景色 (`--accent`) が
  サイドバー背景と近く、選択状態が分かりにくい。
- 停止中セッションの通知文が長く、Resume ボタンが 2 行目に落ちて
  高さを消費する。
- 検索入力欄 (Full-text search / Settings) にアイコンがなく、
  背景色もページと同化して入力欄に見えにくい。

## 変更内容

### 入力欄

- `input::placeholder` / `textarea::placeholder` を常に sans 系
  (`--font-sans`) にする。値が等幅でもプレースホルダは本文フォントになる。
- `Input` / `Textarea` と各ネイティブ `select` の背景を `bg-surface-raised`
  にし、ページ背景から浮かせる。
- Full-text search と Settings の検索欄に検索アイコンを追加し、
  左パディングを揃える。

### エージェント管理

- 「Update catalog」ボタンと独立した Registry ダイアログを削除し、
  「Add agent」ダイアログ内のタブに統合する。
  - **From registry**: カタログ検索、Update catalog、import。
  - **Custom definition**: 従来のカスタム定義フォーム。
- import 後もダイアログを閉じず、その場で複数 import できるようにする。
- Environment variables はラベルと説明文を分離し、
  `KEY=VALUE` / `KEY=@SECRET_KEY` を説明行にまとめて折り返し崩れを解消する。
- ダイアログを `max-h-[calc(100dvh-2rem)]` の flex レイアウトにし、
  ヘッダーとフッターを固定、フォーム / カタログ一覧だけをスクロールさせる。

### 選択状態と停止通知

- サイドバーの選択中ナビゲーションとセッション行を `bg-signal-soft` に変更し、
  signal 色の左インジケータと合わせて背景との差を明確にする。
- デスクトップでサイドバーを閉じたとき、左端に幅 40px のレールと
  「Show sidebar (Ctrl+B)」ボタンを表示し、閉じた後も再度開けるようにする。
  Ctrl+B のトグルは従来どおり動作する。
- 停止通知を「This session is stopped.」に短縮し、
  Resume ボタンと同じ行に固定する。狭い幅では 1 行省略表示にし、
  title 属性で全文を補う。
- 停止中の入力欄プレースホルダも「Send a prompt to resume」に短縮する。

## テスト

- `pnpm --dir ui run check`
- `pnpm --dir ui run lint`
- `pnpm --dir ui run test`
- ローカルノード + Vite dev server を起動し、Light (Bone) で
  Settings / Full-text search / Agents / 停止中セッションを目視確認。
