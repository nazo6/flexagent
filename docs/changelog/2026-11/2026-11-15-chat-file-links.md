# チャット本文のファイルリンクを Files で開く

- **日付**: 2026-11-15
- **対象パッケージ**: `ui`, `docs`
- **対象スクリプト / 設定**: `pnpm --dir ui run check`, `pnpm --dir ui run lint`,
  `pnpm --dir ui exec vitest run`, `pnpm --dir ui run build`, `mise run fmt`
- **関連ドキュメント**:
  - [`docs/07-web-ui-and-pwa.md`](../../07-web-ui-and-pwa.md) §3.2
  - [`docs/changelog/2026-11/2026-11-14-files-tree-preview.md`](./2026-11-14-files-tree-preview.md)

## 背景

エージェント本文は Markdown として描画するが、すべての `<a>` に
`target=_blank` を付けていたため、ファイルへの相対パスリンクは新しいタブで
SPA の 404 になっていた。さらに DOMPurify 3.4 の既定 `ALLOWED_URI_REGEXP` は
`file:` を許可しないため、`file:///...` と `C:\...` は href ごと削除され、
リンクとしてすら描画されなかった。Files ドックには
`dock.openFile(path, mode, scope)` が既にあり、Changes とツールカードから
ファイルのプレビューを開ける状態だった。

## 変更

- **`ui/src/lib/dock/file-links.ts`（新規）**:
  `resolveFileHref(href, baseDir, workspaceRoot)` を追加した。相対 / 絶対パス、
  `file:` URI（localhost / Windows ドライブ / UNC / percent-encoding）、
  `#L12` / `#L12-L20` / `:12` / `:12:5` の行番号を解決する。`http(s)` / `mailto`
  などの URL、`#fragment` のみ、プロトコル相対 `//host`、末尾 `/` のディレクトリ、
  workspace 外のパスは `null` を返して既定動作に任せる。
- **`ui/src/lib/markdown.ts`**: `DOMPurify.sanitize` に `ALLOWED_URI_REGEXP` を
  渡し、既定の許可スキームに `file:` と Windows ドライブ / UNC を追加した。
  `javascript:` / `vbscript:` / `data:` は引き続き拒否する（回帰テスト付き）。
- **`MarkdownText.svelte`**: `fileBaseDir` prop（既定は workspace の
  `localPath`）を追加し、コンテナのイベント委譲でリンクを解決する。直接ノードへ
  接続している場合だけ `dock.openFile(path, "view", "uncommitted", line)` を
  呼び、`preventDefault()` で新しいタブを止める。モバイルは既存の
  `fileOpenVersion` 効果でオーバーレイがプレビューへ切り替わる。
- **Files のプレビュー**: `DockFileTab` に `line` を追加し、永続化する。
  `CodeView` は shiki の transformer で対象行に `fxg-line-focus` を付け、
  その行が中央に来るようスクロールして背景を強調する。Markdown ファイルは
  行指定があるとき Source 表示を既定にする。Markdown プレビュー内の相対リンクは
  プレビュー中のファイルのディレクトリを基準に解決する。
- **`dock/paths.ts`**: `dirname` / `normalizePath` / `isInsidePath` を追加し、
  POSIX と Windows の両方で `.` / `..` を正規化する。

## 不変条件

- リンクの解決は UI だけで完結し、ノードの workspace API を増やさない。
- workspace 外・`.git`・シンボリックリンク脱出の拒否はノード側の access root
  検証にそのまま従う。
- ハブ経由では Files が無効なため、リンクの既定動作を変えない。
- ユーザー発言はプレーンテキストのまま（本文リンクの対象はエージェント本文・
  思考・サブエージェントメッセージ）。

## 検証

- `file-links.test.ts`: 相対 / 絶対 / workspace 外 / `file:` / Windows / UNC /
  percent-encoding / 行番号 / 除外スキーム / ディレクトリ。
- `paths.test.ts`: `dirname` / `normalizePath` / `isInsidePath`。
- `markdown.test.ts`: `file:` とドライブパスの保持、スクリプトスキームの拒否。
- `dock.test.svelte.ts`: 行番号の保存・復元と不正値の破棄。
- `pnpm --dir ui run check` / `lint` / `exec vitest run` / `run build`、
  `mise run fmt`。

## 既知の制約

- インラインコードのパスとツールカードの `locations` はリンク化しない。
- ディレクトリへのリンクは Files のツリーを開かない（既定動作のまま）。
- 存在しないパスへのリンクはプレビューにエラーを表示する。
- リンク先のファイルが Markdown の Preview 表示中でも、行指定がなければ
  従来どおり Preview を既定にする。
