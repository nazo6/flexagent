# Desk デザインシステム導入とテーマパレット

- **日付**: 2026-11-10
- **対象パッケージ**: `ui`
- **対象スクリプト**: `mise run fmt:oxc`, `mise run check:ui`, `mise run test:ui`, `mise run lint:ui`, `mise run build:ui`

## 目的

新規セッション画面を中心に導入した composer / pill / 余白の言語を、セッション・
Projects・Inbox・Search・Settings へ一貫して展開する。同時に、配色は
「デザインの一部」ではなく「差し替え可能なテーマ」として分離し、レイアウト・
タイポグラフィ・角丸・コンポーネント構造を固定したままパレットだけを変更
できるようにする。

## 設計方針: Desk

- **構造は固定、色はテーマ**: type scale・spacing・radius・border・layout・
  motion はテーマで変更しない。パレットは semantic role の値だけを差し替える。
- **タイポグラフィ**: 11px は大文字ラベル専用、12px はメタデータ、13px は UI、
  14px は本文。9px / 10px の使用を廃止した。
- **面**: 1px ヘアラインと `surface` / `canvas` の明度差で構造を作る。
  影は popover / dialog / toast と、キャンバス上を浮く要素だけに限定する。
- **角丸**: pill = full、control = 8px、surface = 12px、dialog = 14px。
- **メタデータ**: 操作・状態だけを pill にし、件数などの受動的な値はテキスト。
  機械の文字列（branch / path / ID / model / 時刻）だけを mono にする。
- **選択状態**: グレーの塗りだけでなく、`--signal` の 2px 左ルールを併用する。
- **ステータス**: 正常 = `--ok`、注意・待機 = `--warn`、エラー = `--danger`。
  色だけで意味を伝えず、ラベルやアイコンを併記する。

## テーマ構成

`ui/src/app.css` に以下の 3 層を定義した。

1. **Design constants**: type scale、spacing、radius、border、layout、motion。
2. **Semantic roles**: `--canvas` / `--surface` / `--sidebar-bg` / `--ink` /
   `--muted-ink` / `--line` / `--chip` / `--signal` / `--ok` / `--warn` /
   `--danger` など。
3. **shadcn mapping**: `--background` / `--card` / `--border` / `--ring` /
   `--primary` などを semantic role へ写像する。

注意点として、shadcn の `--accent` はホバー面の意味で使われているため、
Desk のアクセントは `--signal` という別名にした。これにより既存の
shadcn-svelte コンポーネントの挙動を変えずに配色だけを載せ替えられる。

### プリセット

- **Bone**: 暖色ニュートラル + 琥珀（デフォルト）
- **Graphite**: 冷色ニュートラル + コバルト
- **Moss**: 緑寄りニュートラル + モス

いずれも light / dark を持ち、自由なカラーピッカーは提供しない。任意の色を
許すとコントラストと階層が崩れるため、まずは curated preset に限定した。

## 変更内容

### テーマ適用

- `ui/src/lib/theme.svelte.ts` を追加。
  - `localStorage["fxg:palette"]` に保存（端末ごとの表示設定）。
  - `<html data-palette="...">` を切り替える。
  - `<meta name="theme-color">` を現在の canvas 色へ追従させる。
- `ui/src/app.html` の初回描画前にパレットを適用し、色のフラッシュを防ぐ。
- `ui/static/manifest.webmanifest` の `background_color` / `theme_color` を
  Desk の canvas に更新。
- Settings に **Appearance** タブを追加し、light / dark / system と
  Bone / Graphite / Moss を同じ場所で選択できるようにした。Browser タブは
  pinned sessions のみに整理。

### Shell / 画面

- **Sidebar**: 13px の nav、2px の signal 左ルール、mono の時刻・branch、
  LED 形式の node status、footer の接続メニューと緊急停止の再配置。
- **Session**: ヘッダーの path 行を整理し、Diff / Terminal / Fork / New here /
  Kill を ghost ボタン化。チャット列を 760px に固定。
- **Chat**: アシスタントは吹き出しなし、ユーザーは暖色の淡いブロック。
  Thinking は左罫線、tool call は計器行、権限要求は琥珀の左ルール、
  質問は signal の左ルール。
- **Composer**: 新規・既存で共通。12px 角丸、1px 線、signal のフォーカス
  リング、ink の送信ボタン。停止・再開・権限の通知は warn / danger の
  インライン callout に統一。
- **New session**: 既存の composer 中心レイアウトを維持しつつ、ヒーローの
  アイコンを signal-soft、説明を 13px、callout を warn / danger に統一。
- **Projects**: カードをやめてヘアラインの行に。repo / path は mono、
  branch / node は pill、node binding と worktree は縦ガイド付きの子行。
- **Inbox / Search**: カードをやめて行にし、状態色は左ルールと
  semantic token に限定。Search のハイライトは `--signal-soft`。
- **Diff / Terminal**: 追加・削除色を `--ok` / `--danger` に。ターミナルは
  Desk の暖色ダークに合わせたテーマへ更新（コードハイライトは可読性優先で
  既存の light / dark テーマを維持）。

### コンポーネント

- `Card` は ring + shadow をやめ、1px border + 12px radius に。
- `Button` の outline から shadow を除去。
- line variant の Tabs のアクティブ下線を `--signal` に。
- Dialog / AlertDialog は ring を border + shadow に。
- 9px / 10px のテキストと、`amber-*` / `emerald-*` / `green-*` / `red-*` /
  `yellow-*` などの直接的なパレット指定を UI から除去（terminal の
  固定テーマを除く）。

## 検証

- `mise run fmt:check`
- `mise run lint:ui`
- `mise run check:ui`（svelte-check / tsc）
- `mise run test:ui`（91 tests）
- `mise run build:ui`
- ブラウザで light / dark × Bone / Graphite / Moss、Appearance タブの
  切り替え、リロード後の永続化、`theme-color` の追従を確認。

## 注意・今後

- パレットは現時点で 3 プリセットのみ。カスタム色が必要になった場合は、
  signal の hue だけを選ばせるなど、コントラストと neutral ramp を固定した
  制約付きの生成に留める。
- ターミナルと Shiki のコードテーマはツール面として固定しており、パレット
  切り替えの対象外。
- パレットは端末ごとの localStorage に保存し、node / server へは同期しない。
