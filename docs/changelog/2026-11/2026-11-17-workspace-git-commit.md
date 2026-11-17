# Workspace の Changes からのコミット

- **日付**: 2026-11-17
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-node`, `fxg-api`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run fmt`,
  `cargo test -p fxg-node workspace_file_api_stays_inside_registered_workspace`,
  `cargo clippy --workspace --all-targets`, `mise run lint:errors`,
  `pnpm --dir ui run check`

---

## 概要

Workspace Dock の `Changes` は差分の閲覧だけで、変更を確定するには
ターミナルへ移動する必要があった。`Worktree` スコープに最小限のコミット欄を
追加し、worktree の全変更を stage して 1 コミットを作成できるようにした。

部分 stage、amend、push、ブランチ操作は意図的に対象外とし、Git の複雑な
状態管理を UI に持ち込まない。

## 変更

### プロトコル / Node

- `WorkspaceGitCommitRequest` / `WorkspaceGitCommitResponse` を追加した。
  応答は作成した commit hash、branch、対象ファイル数だけを持つ。
- `NodeRpcRequest::WorkspaceGitCommit` / `NodeRpcResponse::WorkspaceGitCommit`
  を追加し、`requires_local_workspace_access()` の対象に含めた。ハブ経由では
  `FORBIDDEN` になる。
- コミットはフックを含む通常の `git commit` を実行するため、ハブ側の RPC
  タイムアウトを 120 秒にした。
- `workspace_git_commit` は次を行う。
  - メッセージを trim し、空・NUL を含む・64 KiB 超を拒否する。
  - 変更がなければ `INVALID_STATE`（`nothing to commit`）を返す。
  - `git add -A` の後に `git commit -m <message>` を実行する。
- 監査ログに `workspace_git_commit` を追加し、path / commit / branch /
  ファイル数を記録する。メッセージ本文は記録しない。

### Client API / UI

- `POST /api/v1/nodes/:node_id/workspace/git/commit` を追加した。
- `Changes` の `Worktree` スコープで、変更があるときだけ一覧の下に
  コミットメッセージ入力と「Commit all changes」ボタンを表示する。
  成功時はトーストで短縮 hash を出し、`git status` を再取得する。
- 監査ログの表示に `Git commit` を追加した。

## 不変条件

- コミットは worktree の全変更を対象とする（`git add -A`）。`.gitignore`
  されたファイルは含まれない。
- ハブ経由のクライアントはコミットできない（workspace 書き込みはローカル
  接続限定）。
- コミットメッセージは必須で、空文字列は受け付けない。
- push は行わない。リモート認証や force push の判断を UI に持ち込まない。

## 検証

- `cargo test -p fxg-node workspace_file_api_stays_inside_registered_workspace`
  で、変更のコミット、`git log` の subject、コミット後の status が空になる
  こと、空メッセージと変更なしの拒否を確認した。
- `mise run types` で `ui/src/lib/generated/bindings.ts` を更新した。
- `pnpm --dir ui run check` / `mise run fmt:check` を実行する。
