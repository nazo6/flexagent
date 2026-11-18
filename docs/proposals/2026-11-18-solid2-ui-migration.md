# 2026-11-18 Web UI の Solid 2 移行

- **日付**: 2026-11-18
- **ステータス**: `In Progress`（2026-11-18: 方針決定 + Phase 0 の足場を `ui-solid` ブランチに作成）
- **対象パッケージ**: `ui`（新規 `ui-solid`）、`fxg-ui-assets`、`fxg-api`
- **対象スクリプト / 設定**: `mise run build:ui` / `mise run check:ui` / `mise run test:ui` / `mise run types` / `mise run fmt`
- **関連ドキュメント**:
  - [`docs/07-web-ui-and-pwa.md`](../07-web-ui-and-pwa.md)（Desk デザインシステム・PWA）
  - [`docs/06-type-safety.md`](../06-type-safety.md)（型安全方針）
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md)（REST / WS の境界）
  - [`crates/fxg-api/src/static_ui.rs`](../../crates/fxg-api/src/static_ui.rs)（SPA 配信）
  - [`crates/fxg-ui-assets/build.rs`](../../crates/fxg-ui-assets/build.rs)（`ui/build` の埋め込み）

---

## 0. 結論

- Web UI を **Svelte 5 / SvelteKit から Solid 2 へ移行する**。動機は性能ではなく、**型チェックを `tsc` 一本にすること**、**Solid 2 のリアクティブ診断でエージェントの誤りを自己修復させること**、ビルド構成を薄くすること。
- 新アプリは `ui-solid/` に**並走で新規作成**し、Svelte 版は Phase 5 まで出荷可能な状態で維持する。`ui/` を場当たり的に書き換えない。
- ルーターは **`@solidjs/router` 2.0.0**（stable、型付き `paths`）。
- ヘッドレスは **Terracotta 2.0.0**（stable、`solid-js ^2.0.0`）を主とし、**配置（geometry）だけを自前の共通ヘルパー**で持つ。ポップアップの土台に `solid-floating-ui` + `@floating-ui/dom` を使う。
- Drawer / ScrollArea / Skeleton は Terracotta に無いため**自前のプリミティブ**として実装する。
- テストは `@solidjs/testing-library`（RC、API 凍結）+ Vitest、i18n は Paraglide の生成物をそのまま使う。
- 段階は Phase 0（基盤）→ 1（UI ラッパー）→ 2（ストア）→ 3（ルート・画面）→ 4（特殊：ターミナル / diff / PWA）→ 5（切替・Svelte 撤去）。
- **決定済み**（§9.1）: Terracotta 主 + 自前配置 / 長命 `ui-solid` ブランチ / 移行中の機能凍結 / アイコンは `unplugin-icons` / テストは既存の移植のみ / アプリ名は切替時に `ui` へ戻す。

## 1. 目的と非目的

### 1.1 目的

1. **型の通り道を 1 本にする**。`.svelte` の独自コンパイラ・`svelte-check`・`svelte-kit sync` を外し、`tsc --noEmit` と `oxlint` / `oxfmt` だけにする。`docs/06-type-safety.md` の方針とツールを一致させる。
2. **エージェントの誤りをフレームワークに検出させる**。Solid 2 は `[CLEANUP_IN_FORBIDDEN_SCOPE]` のようなコード・発生位置・修復ガイド付きの診断を出す。エージェント記述比率の高い本リポジトリでは、レビュー前に機械が直せる範囲を増やせる。
3. **ビルド構成を薄くする**。SPA を `rust-embed` で配るだけなので、SvelteKit の `$app/*` 抽象・アダプタ・規約は不要。Vite + ルーター + Tailwind に寄せる。
4. **ヘッドレス層を stable に置く**。Kobalte は Solid 2 版が alpha かつ `solid-js@2.0.0-rc.16` 固定。Terracotta 2.0.0 は stable で peer も `^2.0.0`。
5. **移行で得た知見を設計に残す**。ルート定義と型付き `paths` を単一の情報源にし、手書きのパスビルダー（`ui/src/lib/settings-routes.ts`）を減らす。

### 1.2 非目的

- 性能改善。重いのは Monaco / ghostty-web / Shiki で、フレームワーク非依存。バンドルサイズは記録するが目標にしない。
- SSR / プリレンダリング。SPA のまま。
- デザイン変更・機能追加。**移行期間中は Desk の見た目を凍結**する（変更が必要なら両アプリに同じ変更を入れる）。
- API / プロトコルの変更。`fxg-protocol` と生成 bindings は触らない。

## 2. 現状

| 項目                                                            |                                         数 |
| :-------------------------------------------------------------- | -----------------------------------------: |
| `.svelte` ファイル                                              |                        188（約 19,000 行） |
| rune モジュール（`*.svelte.ts`）                                |                          17（約 3,800 行） |
| `ui/src/lib/components/ui`（shadcn ラッパー）                   |                    121 ファイル / 2,537 行 |
| ルートファイル                                                  |                             19（2,069 行） |
| テスト                                                          |                                25 ファイル |
| `bits-ui` を import するファイル                                |   69（**すべて `components/ui/` の内側**） |
| `svelte-sonner` / `mode-watcher` / `vaul-svelte` を使うファイル | 42（うち **32 は `components/ui/` の外**） |
| `@lucide/svelte` のアイコン                                     |                        97 種 / 74 ファイル |
| `bind:` / `class:` / `style:`                                   |                               205 / 94 / 3 |
| `{#each}` / `{#snippet}` + `{@render}`                          |                               91 / 35 + 66 |
| `use:` アクション                                               |                                         13 |
| `$state` / `$derived` / `$props` / `$bindable` / `$effect`      |                 268 / 273 / 173 / 110 / 51 |

構成上の要点:

- SvelteKit は SPA 専用（`+layout.ts` で `ssr = false`、`adapter-static` の fallback `index.html`）。`ui/build` を `rust-embed` が埋め込み、`crates/fxg-api/src/static_ui.rs` が未知パスに `index.html` を返す。**この配信契約はフレームワーク非依存**で、移行後も変更不要。
- `svelte-sonner` / `mode-watcher` / `vaul-svelte` はラッパー層の外（機能コンポーネント）から直接使われている。ここが移行時に最初に詰まる。
- 開発サーバーは `ui/vite.config.ts` の API プロキシ（Host / Origin 書き換え、WS）に依存している。新アプリでも同じ設定が必要。

## 3. 技術選定

### 3.1 フレームワーク: Solid 2

- `solid-js` 2.0.0 / `@solidjs/web` 2.0.0 / `@solidjs/vite-plugin` 3.0.1（すべて 2026-11-17 公開）。
- DOM レンダラは `@solidjs/web`。`solid-js/web` は存在しない。`jsxImportSource` は `@solidjs/web`。
- API は 1.x から縮小・改名されている（後述 §6）。移行時に**必ず 2.0 の書き方**を使う。

### 3.2 ルーター: `@solidjs/router` 2.0.0

- stable（2026-11-17）。ルート定義オブジェクトが単一の情報源で、`paths` が型付き URL ビルダーになる。
- `defineRoute` で `params` がパターンから型付けされ、`matchFilters` で不正なセグメントを 404 に落とせる（**現行の `+page.ts` の `error(404)` を置き換える**）。
- 素の `<a href>` がクライアント遷移し、`aria-current="page"` が自動付与される。`Link` 相当のコンポーネントは無い。
- **不採用**: TanStack Solid Router（Solid 2 版は 2.0.0-rc.10、19 ルートには過剰）、SolidStart（メタフレームワーク廃止）、ファイルベースルーティング（`filesystem-routing@0.5` + `@tsrx/oxc` に依存し、まだ若い）。

### 3.3 ヘッドレス: Terracotta 2.0.0

| 観点             | Terracotta 2.0.0                                                | Kobalte 2.0.0-alpha.4                            |
| :--------------- | :-------------------------------------------------------------- | :----------------------------------------------- |
| Solid 2          | stable（`^2.0.0`）                                              | alpha（`2.0.0-rc.16` 固定）                      |
| API              | 自前合成（Dialog / Popover + Menu / Listbox）                   | Radix 相当（shadcn と 1:1）                      |
| 配置             | **無し**（floating-ui 等を自分で）                              | 内蔵                                             |
| サブメニュー     | 無し                                                            | あり（`Sub` / `SubTrigger`）                     |
| Toast            | `ToasterStore`（ツリー外から投入可）                            | あり（ツリー内）                                 |
| 追加部品         | Command / Tree / TreeGrid / Splitter / ColorScheme / Transition | Accordion / Slider / Switch / Combobox / Menubar |
| エージェント向け | Claude Code プラグイン（skill 同梱）                            | 無し                                             |

**決定**: Terracotta を主とする。

- 理由: alpha を依存から消せる。Toast がストア型で、現行の `toast(...)` 呼び出し（9 箇所）と相性が良い。Command / Tree / Splitter / ColorScheme が、現在自前で持っている検索・フォルダツリー・ドック・テーマにそのまま刺さる。部品ごとの entry point でバンドルが肥大しない。
- 代償: **配置を自前で持つ**、サブメニューが無い、Drawer / ScrollArea / Skeleton が無い。いずれも `ui-solid/src/lib/components/ui` の内側で吸収する（§4.4）。
- Kobalte は**フォールバック**として残す。Terracotta で埋まらない部品が出た場合のみ、その部品単位で検討する（全体を混在させない）。

### 3.4 その他

| 領域                     | 決定                                                                                               |
| :----------------------- | :------------------------------------------------------------------------------------------------- |
| スタイル                 | Tailwind CSS v4 + 既存 `app.css` のトークンをそのまま移植                                          |
| 状態の状態属性           | Terracotta の `tc-*` 属性を Tailwind の任意バリアントで参照（`[&[tc-selected]]:` 等）              |
| テスト                   | Vitest + `@solidjs/testing-library` 1.0.0-rc.2（API 凍結）+ happy-dom                              |
| i18n                     | Paraglide（生成物はフレームワーク非依存）。SvelteKit アダプタ相当のみ捨て、ロケール解決は自前      |
| アイコン                 | `unplugin-icons` + `@iconify-json/lucide`。プロパティ型は Solid 2 向けに `src/env.d.ts` で宣言する |
| ターミナル / diff / 強調 | `ghostty-web` / `monaco-editor` / `shiki` をそのまま使用（フレームワーク非依存）                   |
| Markdown                 | `marked` + `dompurify` をそのまま使用                                                              |
| PWA                      | サービスワーカーは自前（`$service-worker` 相当のプリキャッシュ一覧をビルド時に生成）               |

## 4. アーキテクチャ

### 4.1 ディレクトリ

移行中は `ui/`（Svelte）と `ui-solid/`（Solid）を併存させる。

```
ui-solid/
  index.html
  vite.config.ts        # solid + tailwind + paraglide + API プロキシ
  src/
    index.tsx
    app/router.tsx      # ルート定義（単一の情報源）+ 型付き paths
    app/shells/         # AppShell / SettingsShell
    lib/components/ui/  # 自前ラッパー（Terracotta + floating-ui）
    lib/components/     # 機能コンポーネント（Svelte 版の移植先）
    lib/stores/         # ストア（createSignal / createStore）
    lib/generated/      # specta bindings（移行中は ui/ 側を alias 参照）
    styles/app.css      # Desk トークン（移行中はコピー）
  test/
```

- Phase 5 で `git mv ui-solid ui` し、`ui/build` の契約は変えない。
- 移行中に実バイナリで確認したい場合に備え、`fxg-ui-assets/build.rs` に `FXG_UI_DIR`（既定 `../../ui/build`）を追加する。切替後も開発用の逃げ道として残してよい。

### 4.2 共有するもの / しないもの

- **共有する**: specta bindings、Paraglide のメッセージ定義（`ui/messages`, `project.inlang`）、Desk のトークン、API の契約、開発プロキシの設定内容。
- **共有しない**: コンポーネント実装、ストア実装、サービスワーカー。
- bindings は移行中 `mise run types` の出力先を変えず、`ui-solid` から Vite / tsconfig の alias（`#lib/generated/bindings.ts`）で参照する。二重生成はしない。Phase 5 で出力先を移動する。
- トークンは移行中コピーを持つ。**Desk の変更は移行完了まで凍結**し、やむを得ない場合は両方に同じ変更を入れる（レビューで確認）。

### 4.3 ルーティング

現行 19 ファイルを次の構成に写す。**URL は 1 つも変えない**（ディープリンク・PWA ショートカット・履歴の互換）。

| 現行                                        | 新                                                                 |
| :------------------------------------------ | :----------------------------------------------------------------- |
| `/`                                         | `/`（セッション一覧・新規セッション）                              |
| `/sessions/[id]`                            | `/sessions/:id`                                                    |
| `/projects`, `/search`                      | 同パス                                                             |
| `/settings/{hub,browser,nodes}` + `[tab]`   | 同パス。`matchFilters: { tab: isHubTab }` 等                       |
| `/settings/nodes/[node_id]/[tab]`           | `/settings/nodes/:node_id/:tab?`                                   |
| `+error.svelte`                             | `*404` ルート（シェル内に表示）                                    |
| `+layout.svelte`, `settings/+layout.svelte` | `AppShell`, `SettingsShell`（親ルートの `component` + `children`） |

- `ui/src/lib/settings-routes.ts` の手書きビルダーは `paths.settings.nodes(id)("general")` に置き換え、関数を削除する。
- `goto(...)` は `useNavigate()` または `<a>` に置き換える。`{ replaceState: true }` は `navigate(url, { replace: true })`。
- `page.url` / `page.url.searchParams` は `useLocation()` / `useSearchParams()`。`NewSessionChat` の「URL は入口で 1 回だけ読む」意図は `untrack` で維持する。

### 4.4 UI ラッパー層

`bits-ui` 依存は 69 ファイルすべて `components/ui/` の内側にあるため、**ここが移行の単位**になる。

- Terracotta で直接置き換え: dialog, alert-dialog, tabs, tooltip, popover, select（Listbox）, dropdown-menu（Popover + Menu）, toast。
- **自前で持つ**（Terracotta に無い）:
  - `useAnchoredPanel`: floating-ui を使う共通配置（offset / flip / shift / autoUpdate、`Portal` 併用）。select / dropdown-menu / popover / tooltip で共有。
  - `drawer` / `sheet`: Dialog + CSS（side 配置、オーバーレイ、スワイプは任意）。
  - `scroll-area`: ネイティブ overflow + スクロールバーのスタイル。仮想化が必要になったら別途。
  - `skeleton`, `separator`, `badge`, `card`: 素の HTML + クラス。
  - `dropdown-menu` の `Sub` 系: Popover の入れ子（ポータル内の入れ子は Terracotta が許容）で実装。利用は 4 ファイル。
- 状態属性の読み替え: `data-[state=open]` → `[&[tc-expanded]]:`、`data-[selected]` → `[&[tc-selected]]:`、`data-[highlighted]` → `:focus`（Menu は実フォーカスで動く）。ラッパー内に閉じ込め、機能側には漏らさない。
- スタイルの正は `ui-solid/src/lib/components/ui` の各ラッパー。`data-slot` は維持し、スクショ比較とテストのセレクタを共通化する。

## 5. 移行計画

各 Phase は**長命の `ui-solid` ブランチ**で行い、Phase 5 で一度に `main` へマージする。Svelte 版は常にビルド・出荷可能。

- `main` の変更は週次で `ui-solid` に取り込む（`git merge main`）。Rust / docs 側のコンフリクトは `main` を優先する。`ui/` は凍結中なので競合しない想定。

| Phase          | 内容                                                                                                                            | 完了条件                                                                                              |
| :------------- | :------------------------------------------------------------------------------------------------------------------------------ | :---------------------------------------------------------------------------------------------------- |
| **0 基盤**     | Vite + Solid + Tailwind + Router + Paraglide + bindings alias + 開発プロキシ + CI ジョブ。`PORTING.md`（変換規約）を作成        | `pnpm --dir ui-solid build` が通り、トークンが当たった空シェルが表示される。`tsc --noEmit` がクリーン |
| **1 ラッパー** | 19 プリミティブ + `useAnchoredPanel` + Drawer / ScrollArea / Skeleton / Sub メニュー。`ui/` の Storybook 相当の確認ページを作る | 全ラッパーがキーボードで操作でき、ライト / ダーク / 3 パレットで表示が崩れない                        |
| **2 ストア**   | `*.svelte.ts` 17 ファイルを Solid のモジュールへ。WS / SSE の受信、楽観更新、再接続                                             | 既存テスト相当のユニットテストが通り、イベント適用順序が変わらない                                    |
| **3 画面**     | シェル、ルート 19、機能コンポーネント。依存の葉から。セッション一覧 → 新規セッション → チャット → 設定の順                      | 主要導線が実 API で動く。ルート遷移でフォーカス・スクロールが破綻しない                               |
| **4 特殊**     | ghostty-web、Monaco、Shiki、Markdown、diff、画像、サービスワーカー / Web Push、アイコン一式                                     | ターミナル・diff・Markdown・PWA インストール・通知が Svelte 版と同等                                  |
| **5 切替**     | `git mv ui-solid ui`、`mise` / CI / docs 更新、Svelte 版の削除、`app.css` の一本化、bindings 出力先の移動                       | `mise run check` が通り、リリースビルドで UI が同梱される                                             |

**Phase 0 は完了**（2026-11-18）。`ui-solid` に Vite + Solid + Router + Tailwind + Paraglide + bindings alias + 開発プロキシ + `PORTING.md` を作成し、`tsc --noEmit` と `vite build` がクリーン（100 kB / gzip 37 kB）。Terracotta の Dialog / Toast、アイコン、Paraglide、`ui/` の型が実画面で動作することを確認済み。

### 5.1 並列化の単位

- 1 PR = 1 機能ディレクトリ（例: `settings/nodes`, `chat`, `dock`）。ファイル単位で分割しない。
- 並列は最大 4。**同じディレクトリを 2 人に触らせない**（所有権を PR ごとに宣言）。
- 先に Phase 1 / 2 を終わらせる。ここを飛ばして画面から入ると、各担当が独自のラッパー・ストアを作って後で捨てる。
- レビューは 1 人（メインセッション）。`tsc` / `oxlint` / `vitest` / `vite build` / スクショを PR の必須ゲートにする。

### 5.2 `PORTING.md` に固定する規約（抜粋）

| Svelte 5                   | Solid 2                                                                   |
| :------------------------- | :------------------------------------------------------------------------ |
| `$state(x)`                | `createSignal(x)` / 深いオブジェクトは `createStore`                      |
| `$derived(expr)`           | `createMemo(() => expr)`                                                  |
| `$effect(fn)`              | `createEffect(on(...))`（分割 API。追跡範囲を明示）                       |
| `$props()`                 | 関数引数 + `merge` / `omit`                                               |
| `$bindable()`              | getter / setter props                                                     |
| `{#each list as x (key)}`  | `<For each={list}>`（参照キー）/ `<Index>`                                |
| `{#if} {:else}`            | `<Show when fallback>` / `<Switch><Match>`                                |
| `{#snippet}` / `{@render}` | children props / JSX                                                      |
| `bind:value`               | `value` + `onInput`                                                       |
| `class:active`             | `classList={{ active }}`                                                  |
| `use:action`               | `ref` コールバック（クリーンアップを返す）                                |
| `onMount`                  | `onSettled`（**`onCleanup` を中で呼ばず、クリーンアップを return する**） |
| rune クラス                | ファクトリ関数 + アクセサ                                                 |

禁止事項: `any`、`solid-js/web` の import、`data-[state]` 前提のスタイル、Svelte の再実行モデルを前提にした書き方、`$effect` での派生値計算。

## 6. 検証

- **静的**: 各 PR で `tsc --noEmit`、`oxlint`、`vite build`。追加で Solid の dev 診断がコンソールに出ていないこと。
- **テスト**: 既存 25 ファイルを移植する（新規テストは原則追加しない）。ストアと純関数のユニットテストを主対象とし、ラッパーのキーボード操作テストは既存にある範囲だけ移す。テスト量を増やさないぶん、画面比較と手動チェックリストを必須にする。
- **画面**: 同一ビューポート・ライト / ダーク・Bone / Graphite / Moss のスクリーンショットを Svelte 版と比較（`spike/` の手法を流用）。
- **ゴールデンパス**: 起動 → トークン入力 → セッション一覧 → 新規セッション → ストリーミング → 権限承認 → diff → ターミナル → 設定（ノード / ハブ / ブラウザ） → PWA インストール。
- **回帰の観点**: フォーカス（ダイアログ・メニュー）、スクロール復元、`aria-current`、キーボードのみでの操作、モバイル幅、`window-controls-overlay`。

## 7. リスクと対策

| リスク                                     | 影響                           | 対策                                                                                                            |
| :----------------------------------------- | :----------------------------- | :-------------------------------------------------------------------------------------------------------------- |
| Terracotta 2.0.0 が公開 2 日で、個人メンテ | 部品の端で不具合               | ラッパー層に閉じ込める。バージョン固定 + lockfile。致命的なら Kobalte へその部品だけ差し替え                    |
| 配置を自前で持つ                           | ポップアップの位置ズレ・画面外 | `useAnchoredPanel` を 1 本に集約し、flip / shift / autoUpdate を必ず有効化。ウィンドウ端のテストを追加          |
| サブメニュー / Drawer / ScrollArea が無い  | 実装漏れ・見た目の差           | Phase 1 で先に作る。機能側では使わせず、ラッパー経由に限定                                                      |
| エージェントが Solid 1.x の書き方をする    | 実行時に無反応・警告           | `PORTING.md`、dev 診断、レビュー。`onCleanup` / `createResource` / `solid-js/web` を grep で禁止                |
| `bind:` 205 箇所の判断がぶれる             | 入力の挙動差                   | 「制御 props かローカル状態か」を規約で固定し、ラッパーで吸収                                                   |
| ストアのリアクティブ意味論の差             | 更新漏れ・過剰更新             | Phase 2 を先に完了。イベント適用のユニットテストを先に書く                                                      |
| 重複実装期間の機能追加                     | 2 箇所修正                     | 移行中は `ui/` への機能追加を凍結（バグ修正のみ両方）                                                           |
| PWA / サービスワーカー                     | オフライン・通知の退行         | Phase 4 で SvelteKit の `$service-worker` 相当を自前生成し、実機で確認                                          |
| アイコン（97 種）                          | 見た目の差・ビルドの複雑化     | `unplugin-icons` + Iconify に決定。型は Solid 1 向けで `never` になるため `src/env.d.ts` で自前宣言（対応済み） |
| テストは既存移植のみ                       | 回帰の見逃し                   | 同一ビューポートのスクショ比較と、ゴールデンパスの手動チェックリストを Phase 3 / 4 の完了条件にする             |
| 長命ブランチの差分肥大                     | レビュー困難・`main` との乖離  | 週次で `main` を取り込む。機能凍結。1 PR = 1 機能ディレクトリで小さく保つ                                       |

## 8. 工数

3〜4 エージェント + 1 レビュアー（メインセッション）を想定。並列度は Phase 3 以降で上げる。

| マイルストーン                | 見込み  |
| :---------------------------- | :------ |
| Phase 0〜1（基盤 + ラッパー） | 2〜3 日 |
| Phase 2（ストア）             | 2 日    |
| Phase 3（画面）               | 4〜6 日 |
| Phase 4（特殊）               | 3〜5 日 |
| Phase 5（切替・撤去）         | 1 日    |
| 収束（不具合修正・テスト）    | 1〜2 週 |

「全ファイルが `tsc` を通る」は 4〜6 日、「リリース品質」は 2〜3 週の見込み。**Svelte 版を出したまま進められるため、停止期間はゼロ**。

## 9. 決定事項と残る実装判断

### 9.1 決定（2026-11-18）

| 項目             | 決定                                                                      |
| :--------------- | :------------------------------------------------------------------------ |
| ヘッドレスの土台 | **Terracotta 2.0.0 を主**。Kobalte は部品単位のフォールバックのみ         |
| ブランチとマージ | **長命 `ui-solid` ブランチ**。Phase 5 で一度に `main` へマージ            |
| 移行中の機能追加 | **凍結**。バグ修正のみ、必要なら両方に入れる                              |
| アイコン         | **`unplugin-icons` + `@iconify-json/lucide`**。型は `src/env.d.ts` で補う |
| テスト           | **既存 25 ファイルの移植のみ**。新規テストは原則追加しない                |
| アプリ名         | `ui-solid` のまま進め、Phase 5 で `git mv ui-solid ui`                    |

### 9.2 実装中に決める（所有: メインセッション）

1. サブメニュー: Popover の入れ子で足りるか、自前のフォーカス移動が要るか（利用 4 ファイルで試す）。
2. `ScrollArea`: ネイティブ overflow で済ませるか、仮想化まで含めるか（大量ファイルのツリーで必要になる可能性）。
3. サービスワーカーのプリキャッシュ生成: Vite プラグインか、ビルド後スクリプトか。
4. `FXG_UI_DIR` を `build.rs` に足すタイミング（実バイナリでの先行確認をいつ始めるか）。
5. `docs/07-web-ui-and-pwa.md` の更新範囲（Desk の不変条件は維持し、実装手段の記述だけ差し替える）。

## 10. 却下した選択肢

- **Svelte 継続**: 最も安全だが、`tsc` 一本化と診断の利得が得られない。移行コスト以外の downside が小さいため、本案を採る。
- **Kobalte alpha を主とする**: API は shadcn と 1:1 で最速だが、`solid-js@2.0.0-rc.16` 固定の alpha を土台に約 1.9 万行の移行を載せるのは、切替後に peer 不一致と破壊的変更を抱え込む。Terracotta で置換できる範囲では置換する。
- **TanStack Solid Router**: 型安全性は高いが Solid 2 版が RC で、19 ルートには機能過剰。`@solidjs/router` 2.0 の `paths` で十分。
- **SolidStart**: 廃止方針。SPA には不要。
- **ファイルベースルーティング**: `filesystem-routing@0.5` と `@tsrx/oxc` に依存。19 ファイルなら config オブジェクトのほうが読みやすく、型も直接効く。
- **`ui/` の in-place 書き換え**: 途中で動かない期間が生じ、Svelte 版を出荷できなくなる。並走新規作成を採る。

## 11. 参考（検証済みの事実）

`spike/solid2/` で確認した内容（コミット対象外）。

- Solid 2.0.0 + `@solidjs/vite-plugin` 3.0.1 + Tailwind v4 で `tsc` クリーン、ビルド約 250 ms。
- `ui/src/app.css` をほぼ無改造でコピーでき、ライト / ダーク / パレットがそのまま効く。
- Kobalte alpha.4 は stable `solid-js` で動作する（peer 警告のみ）が、peer は rc.16 固定。
- `lucide-solid` 1.75.0 は `solid-js/web` を import しており、Solid 2 ではビルドできない。
- Solid 2 の診断が `[CLEANUP_IN_FORBIDDEN_SCOPE]` をファイル位置・修復ガイド付きで出し、修正まで更新を停止する。
- `@solidjs/router` 2.0.0 で、素の `<a>` のクライアント遷移、`aria-current` 自動付与、`matchFilters` による 404、必須 param の型エラーを確認。
- 参考バンドル: ルーター込みで 194 kB（gzip 66 kB）。

関連リンク:

- Solid 2.0 リリースノート: <https://www.solidjs.com/blog/solid-2-0-the-long-hold>
- Terracotta: <https://terracotta-docs.vercel.app/> / <https://github.com/lxsmnsyc/terracotta>
- `@solidjs/router` 2.0: <https://docs.solidjs.com/solid-router>
