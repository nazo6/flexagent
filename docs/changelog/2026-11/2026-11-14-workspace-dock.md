# セッション右ペインの Workspace Dock 化（Phase 0）

- **日付**: 2026-11-14
- **対象パッケージ**: `ui`
- **対象スクリプト / 設定**: `pnpm --dir ui run check`, `pnpm --dir ui run lint`,
  `pnpm --dir ui exec vitest run`, `pnpm --dir ui run build`, `mise run fmt`
- **関連ドキュメント**:
  - [`docs/proposals/2026-11-14-workspace-dock.md`](../../proposals/2026-11-14-workspace-dock.md)
  - [`docs/07-web-ui-and-pwa.md`](../../07-web-ui-and-pwa.md) §3.2, §4

> 2026-11-14 のレイアウト更新で、外側は固定ツール、可変タブは Files 内へ移した。
> 現在の構造は [`2026-11-14-files-tree-preview.md`](./2026-11-14-files-tree-preview.md)
> を、Terminal の複数シェルは
> [`2026-11-14-terminal-tabs.md`](./2026-11-14-terminal-tabs.md) を正とする。

## 背景

右ペインは `diff | terminal` を `SessionPanelHost` が `{#if}` で分岐するだけの構造で、
タブもツールごとの状態も持っていなかった。さらにセッション切替時に
`activePanel` を `null` に戻していたため、workspace スコープにしたターミナルを
実質セッション単位で閉じていた。

## 変更

- **ドックの状態（`ui/src/lib/stores/dock.svelte.ts`）**:
  - workspace（`node_id` + canonical `local_path`）ごとにタブ・アクティブタブ・
    開閉・幅を保持するストアを追加した。
  - タブの同一性は `tool + resource`。singleton ツールは既存タブを再利用する。
  - タブ上限は 8。workspace の保存は直近 12 件までに制限し、未知のツールや
    重複タブを含む壊れたエントリは読み込み時に捨てる。
  - タブのメタデータと幅だけを `localStorage["fxg:workspace_dock"]` に保存する。
- **ツールレジストリ（`ui/src/lib/dock/registry.ts`）**:
  - Changes / Terminal を launcher + singleton として定義した。ツールの追加は
    レジストリとホストの描画分岐で行う。
- **UI（`ui/src/lib/components/dock/`）**:
  - `DockHost` / `DockTabBar` / `DockRail` / `DockToolsSheet` を追加した。
  - `ChangesTool` は既存の `DiffPane` を、`TerminalTool` は `TerminalView` を包む。
  - タブは `role="tablist"` / `role="tab"` とし、左右キーで移動できる。
  - レールはツールの起動器に変え、同じツールの再クリックでドックを畳む。
  - モバイルはセッションヘッダーの Changes / Terminal アイコンから 1 タップで開く。会話の上に背景を暗くしたオーバーレイとして表示し、パネルは余白と影で浮かせる。背景タップ・右上の閉じるボタンで畳み、背後の会話は `inert` にする。ツールシートはドック内の `+` から開く。
- **セッション画面（`ui/src/routes/sessions/[id]/+page.svelte`）**:
  - `activePanel` とパネル幅のローカル state を削除し、ドックストアへ移した。
  - セッション切替時は workspace が変わったときだけタブセットを入れ替える。
  - 幅は `clamp(22rem, N%, 75%)` で保持し、ドラッグ中はライブ更新、
    ポインタアップ時に永続化する。
- **ターミナル（`TerminalView.svelte`）**:
  - `active` prop を追加した。ドックが開いている間、非アクティブのターミナルは
    `hidden` でマウントを維持し、PTY を detach しない。
  - 再アクティブ時に `fit()` と resize を送る。非アクティブ時はフォーカスを外し、
    隠れた PTY へキー入力が流れないようにする。
- **削除**: `session-panels.ts` / `SessionPanelHost.svelte` /
  `SessionToolRail.svelte` / `SessionToolsSheet.svelte`。ラッパーは残さず、
  ドックへ一本化した。
- **i18n**: ドックのタブ名・閉じる・リサイズのメッセージを追加し、
  旧パネル名のメッセージを削除した。

## 不変条件

- ドックの状態キーは `(node_id, canonical local_path)`。セッションは来歴であり、
  タブの寿命を決めない。
- 同じ workspace のターミナルは 1 本。ドックを閉じたときだけ detach する。
- 最後のタブを閉じるとドックを畳む。レールから開き直せる。
- デスクトップとモバイルのスロットは片方だけをマウントし、
  1 つの `TerminalView` は最大 1 つの PTY に接続する。

## 検証

- `dock.svelte.ts` のストアテストを追加した（workspace 切替、singleton、
  永続化、壊れたエントリ、幅の clamp、workspace の上限）。
- `pnpm --dir ui run check` / `lint` / `vitest run` / `build` が成功した。

## 既知の制約

- Files / File タブ、git status、ファイルの読み書き、エディタは Phase 1 以降。
- Changes の選択状態はドックを開いている間は保持されるが、リロードでは復元しない。
- 旧 `fxg:session_side_panel_width` は引き継がない（新しいキーで初期幅から始まる）。
