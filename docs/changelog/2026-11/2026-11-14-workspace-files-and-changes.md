# ワークスペースのファイル閲覧と Changes 連携（Workspace Dock Phase 1）

- **日付**: 2026-11-14
- **対象パッケージ**: `fxg-protocol`, `fxg-node`, `fxg-hub`, `ui`, `docs`
- **対象スクリプト / 設定**: `mise run types`, `cargo test -p fxg-node`,
  `cargo clippy`, `pnpm --dir ui run check`, `pnpm --dir ui run lint`,
  `pnpm --dir ui exec vitest run`, `pnpm --dir ui run build`, `mise run fmt`
- **関連ドキュメント**:
  - [`docs/proposals/2026-11-14-workspace-dock.md`](../../proposals/2026-11-14-workspace-dock.md)
  - [`docs/03-protocol-and-api.md`](../../03-protocol-and-api.md) §3.1
  - [`docs/07-web-ui-and-pwa.md`](../../07-web-ui-and-pwa.md) §3.2

## 背景

Phase 0 で右ペインを workspace スコープのタブ式ドックにした。Phase 1 では
Files ツリーと File タブを追加し、Changes の一覧からファイルの diff を開ける
ようにした。ファイルアクセスは当初の決定どおり、直接ノードへ接続した場合のみ
許可する。

## 変更

- **プロトコル（`fxg-protocol`）**:
  - `WorkspaceFsQuery` / `WorkspaceFsEntry` / `WorkspaceFsListResponse`、
    `WorkspaceFileContent` / `WorkspaceFileView`、
    `WorkspaceGitStatusResponse` / `WorkspaceGitEntry` / `WorkspaceGitFileStatus`、
    `WorkspaceGitDiffQuery` を追加した。
  - `NodeRpcRequest` に `WorkspaceFsList` / `WorkspaceFsFile` /
    `WorkspaceGitStatus` / `WorkspaceGitDiff` を、`NodeRpcResponse` に対応する
    結果を追加した。`requires_local_workspace_access()` でハブ経由を判別する。
  - メッセージ集合が増えるため `PROTOCOL_VERSION` を 4 に上げた。旧ノードは
    `NodeHello` で拒否される。
- **ノード（`fxg-node/src/workspace.rs`）**:
  - 登録済み workspace（`project_node_bindings` / `project_path_links` /
    セッションの作業ディレクトリ）から access root を解決する。access root は
    Git の worktree toplevel、非 Git なら workspace パス。
  - パスは canonical path の prefix で検証し、`.git` とシンボリックリンク経由の
    脱出を拒否する。ディレクトリ一覧は `.git` を除外し、ディレクトリ優先で
    ソート、2000 件で打ち切る。
  - ファイルは 2 MiB まで読み、NUL / 不正 UTF-8 をバイナリ、超過を
    `TooLarge` として正常系で返す。テキストには SHA-256 ハッシュを付ける。
  - `git status --porcelain=v2 --branch -z` を解析し、branch / ahead / behind、
    rename の旧パス、untracked / conflict を返す。上限は 5000 件。
  - `git diff` に単一ファイル指定を追加した。untracked ファイルは
    `git diff HEAD` に出ないため、内容から新規ファイルの合成 diff を作る。
  - `AuditSource` に `via_hub` を追加し、ハブ経由の workspace RPC は
    `FORBIDDEN` にする。`NodeError::Forbidden` / `NotFound` を追加した。
- **ハブ / 共通ルーター（`fxg-hub`）**:
  - `GET /api/v1/nodes/:node_id/workspace/fs/list`・
    `.../fs/file`・`.../git/status`・`.../git/diff` を追加し、`NodeRpc` へ中継する。
- **UI（`ui`）**:
  - `DockToolId` に `files` と `file` を追加した。Files は launcher + singleton、
    File はパスごとのリソースタブ。`DockTab` に `mode`（`view` / `diff`）を追加し、
    永続化する。
  - Files ツリー: 遅延読み込み、git status バッジ、隠しファイル / 重い
    ディレクトリの表示切替、変更のみ一覧、シンボリックリンク表示。
  - File タブ: Shiki の読み取り専用 View、単一ファイル Diff、バイナリ /
    大きすぎる / 削除済みの案内。Git status バッジ、Markdown の Preview / Source
    切替、長い行でもコード背景がスクロール幅に追従する表示、タブの中クリック
    クローズを追加した。
  - Changes: Session / Worktree / Branch の 3 スコープ。行クリックで File タブを
    Diff モードで開く。ハブ接続時は従来のインライン Diff にフォールバックする。
  - モバイルヘッダーとデスクトップレールに Files を追加した。
- **テスト**:
  - `git status` パーサー（通常 / untracked / rename / conflict / detached）、
    `.git` 拒否、ディレクトリ一覧、未追跡 diff、workspace 境界
    （登録外 / サブディレクトリ / シンボリックリンク脱出）を追加した。

## 不変条件

- workspace ファイル API は登録済み workspace の access root 配下だけを扱い、
  worktree / メイン repo / 別 worktree をまたがない。
- `.git` は一覧・読み取り・diff の対象外。
- workspace API は直接ノードへ接続した場合のみ。ハブ経由は拒否する。
- 読み取りは UTF-8 / 2 MiB、`git status` は 5000 件、一覧は 2000 件の上限を持つ。
- Phase 1 は読み取り専用。ファイルの作成・保存・削除・リネームは行わない。

## 検証

- `cargo test -p fxg-node`（workspace / git のテストを含む）
- `cargo clippy -p fxg-protocol -p fxg-node -p fxg-hub --all-targets -- -D warnings ...`
- `pnpm --dir ui run check` / `lint` / `vitest run` / `build`
- `mise run fmt:check`

## 既知の制約

- ファイルの編集・保存は Phase 2。
- `.gitignore` の完全な解釈はせず、UI が既定の重いディレクトリを隠す。
- 画像 / Markdown プレビュー、チャットからの行ジャンプ、ファイル監視は未対応。
- ハブ経由では Files / File タブを利用できない（Changes は従来の DiffPane）。
