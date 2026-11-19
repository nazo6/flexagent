# 2026-11-19 Solid 2 Web UI の不具合修正・リアクティビティ健全化

- **日付**: 2026-11-19
- **対象パッケージ**: `ui`（補助的にルート設定・`docs/`）
- **対象スクリプト / 設定**: `mise run check:ui` / `mise run test:ui` / `mise run lint:ui` / `mise run build:ui` / `mise run fmt`
- **関連提案**: [`docs/proposals/2026-11-19-solid2-ui-fixes-and-hardening.md`](../../proposals/2026-11-19-solid2-ui-fixes-and-hardening.md)

## 概要

Solid 2 の「書き込みは次のフラッシュまで見えない」「`createEffect` は戻り値を比較しない」「`<For keyed>` は参照で照合する」という仕様に起因する不具合を修正し、モーダル / ポップアップ / フォームのプリミティブを共通化・堅牢化した。

## 主な変更

### 機能破壊の修正

- 外部セッション取込ダイアログのノード解決を `load(force, nodeId)` に変更し、未フラッシュの `selectedNodeId()` 読み取りを廃止した。`HISTORY_UNAVAILABLE` 時はダイアログ内の「履歴なしで取り込む」ボタンで再試行できる。
- 新規セッション画面の capability probe を、対象エージェントの保存済み設定から開始するよう変更した。`persistPrefs({ agent, mode })` は確定値を保存する。
- `/settings/nodes` を index リダイレクト扱いから外し、Hub でオンラインノードが 0 台でも設定ヘッダーと `no_online_nodes` カードを表示するようにした。末尾スラッシュも正規化する。
- `SessionPage` の `loadOlder` は DOM 計測前に `flush()` し、スクロール位置を維持する。リサイズリスナーの `pointercancel` / アンマウント時解除を追加した。
- セッション切替時に `resumeRequired` / `followOutput` / `showScrollBottom` をリセットする。

### リアクティビティ

- `createMemo` の値比較（`equals`）で effect の再接続を抑止: `FilesTool`, `FilePreview`, `AgentUsage`, `SessionHeader`, `MonacoDiff`, `ChangesTool`, `SettingsCatalog`, `PairingPanel`。
- `<For>` に安定キーを付与: サイドバー（プロジェクト / セッション）、プロジェクト一覧、設定ピッカー、ツールコール、監査ログ。重複しうる文字列配列は `keyed={false}`。
- `ChatTimeline` の行コンテキストを accessor 化し、`CodeView` / `ProvisioningLogCard` の DOM ref を signal 化した。
- `ElicitationCard` は数値・カンマ区切り配列の編集中ドラフトを保持し、`-` や `1.`、末尾カンマを入力できる。
- `SyncStore` のタイムラインを `ReactiveMap` 化し、WS の stale ソケットガード・再接続時のストリーム破棄・refresh の in-flight ガードを追加した。`SessionTimeline` は配列をシャローコピーして通知する。
- `ReactiveCell.peek()` と `ReactiveMap.set` の `Object.is` ガードを追加し、`ownedWrite: true` を撤去した（cleanup は Solid 2 の `enterDisposal` で許可される）。

### UI プリミティブ

- `Dialog` / `AlertDialog` / `Sheet` のスクロールロックを参照カウント付き `modal.ts` に共通化し、独自のフォーカス復元を削除して Terracotta の `useFocusStartPoint` に一本化した。`AlertDialog` の背景クリックでは閉じない。
- `createControllable` は制御 / 非制御を動的に判定し、`value` / `onChange` / `default` を accessor で受け取る。
- `Select` / `Popover` / `DropdownMenu` / `Tooltip` のトリガーは `type="button"` を既定にした（`<form>` 内の誤送信防止）。
- `Tabs` は `defaultValue` の非制御切替、`tc-disabled` スタイル、非選択パネルの非表示を修正した。
- `Sheet` は `Portal` 化、`Drawer` は `side="bottom"` 既定に修正した。`Input` / `Textarea` は非制御値を上書きしない。
- ポップアップの外側クリック判定をパネル所有ツリー（`PanelTreeContext`）に変更し、Popover 内の Select などネストしたパネルで外側クリックが効くようにした。`position.ts` は同値時の再配置抑止、接続待ちリトライ、矢印の 4 辺クリア、`cancelled` ガードを追加した。
- Toast は `dismiss()` でタイマーを解除し、アクションボタン / 閉じるボタン / 既定アイコン色を持つ。

### テーマ・ターミナル・Markdown

- `index.html` のインラインスクリプトが `.dark` をペイント前に適用する。`mode.ts` は `matchMedia` / `document` ガード、`createMemo`、`storage` イベント同期を持つ。Shiki は `mode()` に連動する。
- `GhosttyWebAdapter` に `#disposed` / `#mountToken` を追加し、`TerminalView` は DOM 接続後にマウントする。
- `MarkdownText` の 80ms スロットルは cleanup でタイマーを消さず、最新テキストを保持する。

### 開発基盤

- `ui/vitest.config.ts` に `restoreMocks` と Solid 診断を失敗にする setup を追加した。
- `ui/.oxlintrc.json` の `ignorePatterns` を `build/**` に直し、`vitest` プラグインを有効化した。`Dockerfile` は `ui/pnpm-workspace.yaml` をコピーする。
- UI プリミティブ（`controllable` / `position` / `input` / `tabs` / `select` / `dropdown-menu` / `dialog`）の単体テストを追加した。

## フォローアップ（同日対応）

- `NewSessionChat.tsx`（約 1,900 行）を機能単位に分割した。プロジェクト / ノード / パス / エージェント解決と capability probe は `use-new-session-target.ts`、ピッカーは `AgentPicker` / `DirectoryPicker` / `EnvironmentPicker` / `ModelSelectors` / `ProjectPicker`、ヘッダーと通知は `NewSessionHeader.tsx`、モード解決・作成リクエスト生成・provisioner テスト・パスラベルは純関数モジュールへ移した。画面本体（フォーム状態とハンドラ）は約 850 行。
- `src/service-worker.js` の base64url デコードは app と同じ実装（`src/service-worker/format.ts`）に統合した。SW は classic script として出力するため、ビルド時に import した関数本体を `sw.js` へインライン展開し、未定義の export があればビルドを失敗させる。通知本文は同じモジュールの `toNotificationBody()` で整形する。
- コンポーネント独自のコールバック prop を camelCase に統一した（`onselect` → `onSelect` など 22 個）。命名規則は [`ui/CONVENTIONS.md`](../../../ui/CONVENTIONS.md) に追記した。

## 残タスク

- 上記フォローアップ内の主要画面のライト / ダーク / テーマ別スクリーンショット巡回（[`docs/backlog.md`](../../backlog.md) §8）は引き続き手動確認項目。
