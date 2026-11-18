# Solid 2 の意味論に合わせた UI の整理

- **日付**: 2026-11-18
- **対象パッケージ**: `ui`
- **対象スクリプト / 設定**: `pnpm --dir ui run check` / `test` / `lint`、`mise run fmt`

## 概要

移行時に残った「Solid 2 の書き方から外れた箇所」を、挙動を変えずに整理した。
状態層（`ReactiveCell` / `ReactiveMap` / `ReactiveSet`）は意図的に残している
（理由は後述）。

## 主な変更

- **コンポーザの下書きを編集箇所で書く**。`text` / `attachments` を
  `createEffect` で draft ストアへミラーしていたのをやめ、テキストは
  `Composer.updateText` / `NewSessionChat.updatePromptText`、添付は
  `createComposerAttachments({ onChange })` を通す。キーストロークごとに
  2 度目のリアクティブパスが走らなくなった。
- **`onReady` をレンダー本体から `onSettled` へ**。`Composer` は親
  （`SessionPage`）の変数をコンポーネント本体で書いていた。Solid 2 の
  「所有スコープ内の書き込み」に当たるため、settle 後に 1 回渡す。
- **タイムラインの getter props を値に**。`ChatTimeline` の
  `TimelineContext` は 7 個のアクセサを受け渡していた。`<For keyed>` が
  既に reducer の安定キーで行を保持しているため、memo で作った値オブジェクト
  を渡してもストリーミング中に行は再マウントされない。memo の各フィールドは
  値が変わったときだけ更新される。
- **ライフサイクル用の `createEffect(() => true, ...)` を `onSettled` に**
  （`SessionHeader` の resize リスナー、`SessionPage` のスクロールフレーム
  破棄）。`onSettled` は cleanup を返す形が正で、`onCleanup` を直接呼ばない。
- **`ownedWrite` は残す**。外せるか検証したが、`SessionPage` が unmount 時に
  effect の cleanup から `setActiveSession(null)` を書くため必須だった
  （`sync-effect.test.ts` が検出）。理由を `reactive.ts` に明記した。
- **oxlint の `unicorn/consistent-function-scoping` を無効化**。Solid の
  コンポーネント本体はインスタンスごとに 1 回しか走らないため、このルールの
  前提（呼び出しごとに再生成される）が成り立たない。巻き上げると `props` が
  スコープ外になる。理由は `ui/CONVENTIONS.md` に記録した。
- `ui/CONVENTIONS.md` に **Reactivity rules**（ミラー禁止・書き込み可能スコープ・
  props は値・effect の用途）を追加し、`classList` の 1.x 表記を
  `class={{ ... }}` に直した。

## 非同期ロードを async `createMemo` へ寄せなかった理由

移行時に effect で書かれていたロードのうち、非同期計算へ機械的に移せるものを
調査したが、**この UI では失敗時の見せ方が「直前のスナップショットを残して
toast」であり、`<Errored>` は内容を置換する**ため、移すと挙動が変わる。
加えて次の 3 種類は入力の純関数ではない。

- `AgentUsage`: 定期ポーリング + 手動更新 + 失敗時に前回値を保持
- `FolderPicker`: クリックによるディレクトリ移動（イベント駆動）
- `AuditLog` / `DiagnosticsPanel`: 手動更新の失敗で表を残したまま toast

そのため、effect のまま残し、判断を `ui/CONVENTIONS.md` と
`docs/backlog.md` に記録した。エラー表示をインライン（内容置換）に変える
決定をすれば、画面単位で `<Loading>` / `<Errored>` へ移行できる。

## 検証

- `pnpm --dir ui run check`: 成功（`tsc --noEmit`）
- `pnpm --dir ui run test`: 26 ファイル / 216 テスト成功
- `pnpm --dir ui run lint`: 警告 4 件（`no-shadow` 3・service worker の
  `toSorted` 1。いずれも今回の変更と無関係）
