# Web ターミナルの IME 入力と 1 パネル 1 PTY の修正

- **日付**: 2026-11-14
- **対象パッケージ**: `ui`, `docs`
- **対象スクリプト / 設定**: `pnpm --dir ui run check`, `pnpm --dir ui run lint`,
  `pnpm --dir ui run test`, `mise run fmt`
- **関連ドキュメント**:
  - [`docs/07-web-ui-and-pwa.md`](../../07-web-ui-and-pwa.md) §4

## 背景

1. **IME を有効にするとターミナルへ一切入力できなかった**。Enter だけは反応するため、
   WebSocket や PTY 側の不具合に見えていた。
2. **ターミナルを開くたびにシェルが 2 つ起動していた**。ユーザーの `~/.zprofile` が
   `eval $(ssh-agent)` を実行するため、画面に `Agent pid ...` が 2 行現れていた。

## 原因

1. `ghostty-web` 0.4.0 は、入力ターゲットである隠し `textarea` ではなく
   `contenteditable` なラッパーにフォーカスし、そのラッパーで `beforeinput` を
   `preventDefault()` している。`beforeinput` はバブリングするため、`textarea` 側で
   挿入されるはずのテキスト（IME の確定・変換中テキスト・IME 経由の英字入力）も
   すべて打ち消されていた。さらにキーハンドラの先頭で
   `event.isComposing || event.keyCode === 229` を無条件に破棄するため、
   IME（fcitx5 / SKK 等）が有効な状態では文字キーがすべて `keyCode 229` で届き、
   1 文字も `onData` に到達しなかった。Enter は IME を通らず `keyCode 13` で届くため
   動いて見えていた（upstream の #119 / #120 / #190 が未マージ）。
2. セッション画面はサイドパネルのスロットをデスクトップ用（`lg:flex`）とモバイル用
   （`lg:hidden`）の 2 系統で描画していた。CSS で片方は非表示になるが、両方の
   `SessionPanelHost` がマウントされ、`TerminalView` がそれぞれ PTY を spawn していた。
   監査ログ (`audit_logs.action = 'pty_spawn'`) でも、1 回のオープンに対して
   同一ミリ秒の spawn が 2 件記録されていた。

## 変更

- **`GhosttyWebAdapter`（`ui/src/lib/components/terminal/ghostty-adapter.ts`）**:
  - フォーカスを隠し `textarea` へ移す（`focus()` と `focusin` / `mousedown` の転送）。
    IME はこの `textarea` 上で変換する。
  - `beforeinput` を `textarea` で `stopPropagation()` し、ラッパーによる
    `preventDefault()` が挿入を打ち消さないようにする。
  - 挿入されたテキストを `input` から読み、送信後は `textarea` を空にする。
    `compositionend` が先に来て `input` が来ないブラウザでは、マイクロタスクで
    `textarea` の残りを送る（両方来ても 1 回だけ送る）。
  - 変換中テキストは `compositionstart` / `compositionupdate` でカーソルセルに描画する。
    ラッパーを `position: relative` にしてセル寸法（`canvas` の実寸 ÷ `cols` / `rows`）
    から位置を求める。
  - 変換系イベントはラッパーへ伝播させない（確定文字列の送信経路をアダプタに一本化する）。
  - `deleteContentBackward` など、IME が `beforeinput` として報告する編集キーは
    制御文字に変換して送る。
- **セッション画面（`ui/src/routes/sessions/[id]/+page.svelte`）**:
  - `matchMedia("(min-width: 64rem)")` でデスクトップ / モバイルのスロットを切り替え、
    片方だけをマウントする。
- **テスト**:
  - `ui/src/lib/components/terminal/ghostty-adapter.test.ts` を追加し、
    フォーカスが `textarea` に移ること、`beforeinput` がラッパーへ届かないこと、
    挿入テキストの送信、変換中テキストの保持、確定がイベント順によらず 1 回だけ
    送られることを検証する。

## 不変条件

- 1 つの `TerminalView` インスタンスは最大 1 つの PTY にしか接続しない。
- IME のテキストは `textarea` から `onData` へ至る経路を唯一とする
  （キー入力・ペースト・IME 確定のいずれも二重送信しない）。
- 端末へ文字を送るのは、`ghostty-web` のキーハンドラか、アダプタの `input` /
  `compositionend` のどちらか一方だけ。

## 既知の制約

- 変換中のプレビューはターミナルと同じフォントでカーソル位置に描画するが、
  選択範囲や折り返しの扱いは上流の IME 対応（#190）が入るまでの暫定実装。
- デスクトップ / モバイルの境界（`64rem`）をまたいでウィンドウをリサイズすると、
  パネルが再マウントされ PTY も作り直される。
- `~/.zprofile` などでログインシェルごとに `ssh-agent` を起動している場合、
  ターミナルを開くたびにエージェントが増える。これはシェル設定側の挙動であり、
  本修正の対象外（PTY は 1 つに戻るため `Agent pid` の行も 1 つになる）。
