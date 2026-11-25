# Files のファイル名クイックオープン

- **日付**: 2026-11-25
- **対象パッケージ**: `fxg-protocol`, `fxg-node`, `fxg-api`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run fmt`, `cargo test -p fxg-node --lib workspace`,
  `cargo clippy -p fxg-node -p fxg-api --all-targets`, `pnpm --dir ui run check`

---

## 概要

Files ツールのツリーはディレクトリを展開しながら遅延読み込みするため、目的の
ファイルが深い場所にあると辿るのが遅かった。ツリー上部にフィルタを置き、
workspace 全体の相対パスを部分一致で検索して直接プレビューで開けるようにした。

## 変更

### `fxg-protocol`

- `WorkspaceFsSearchQuery` / `WorkspaceFsMatch` / `WorkspaceFsSearchResponse` を
  追加し、`NodeRpcRequest::WorkspaceFsSearch` と対応する応答を追加した。
  `specta` の型を再生成済み (`ui/src/lib/generated/bindings.ts`)。

### `fxg-node`

- `workspace::search_workspace_files` を追加した。workspace のアクセスルートを
  起点に、Git リポジトリでは
  `git ls-files -z --cached --others --exclude-standard` の結果を、
  非 Git では上限付きのディレクトリ走査を使う。
- 相対パスの部分一致 (大文字小文字を無視) で絞り、ファイル名の完全一致 →
  前方一致 → 部分一致 → パスのみ一致の順、次に短いパス順に並べる。
  返す件数はサーバー側で最大 200 件に丸める。
- `.git` / `node_modules` / `target` などの重いディレクトリは非 Git 走査で
  スキップし、走査数にも上限を設けた。絶対パスはアクセスルートからの結合で
  作り、`..` を含む相対パスは除外する。

### `fxg-api`

- `GET /api/v1/nodes/:node_id/workspace/fs/search` を追加し、ハブ経由では
  対象ノードへ中継する。

### `ui`

- `ApiClient.workspaceFsSearch` を追加した。
- Files ツリーの上部にフィルタ入力を置き、180ms デバウンスで検索する。
  入力中はツリーの代わりに相対パス付きの候補を表示し、選択すると
  既存のプレビュータブで開く。0 件 / 検索中 / 上限到達の表示を持つ。

## 検証

- `cargo test -p fxg-node --lib workspace` (ランキング、非 Git 走査、
  `git ls-files` の `.gitignore` 適用)
- `cargo clippy -p fxg-node -p fxg-api --all-targets -- -D warnings`
- `pnpm --dir ui run check` / `pnpm --dir ui run test`
- 手動: 実ブラウザで Files を開き "workspace" で検索、候補から
  `crates/fxg-node/src/workspace.rs` を開いてプレビュー表示を確認。
