# Workspace の Pull / Push（fast-forward 限定）

- **日付**: 2026-11-17
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-node`, `fxg-api`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run fmt`,
  `cargo test -p fxg-node workspace_git_push_and_pull_are_fast_forward_only`,
  `cargo clippy --workspace --all-targets`, `mise run lint:errors`,
  `pnpm --dir ui run check`

---

## 概要

`Changes` で差分を見てコミットできるようにしたが、upstream との同期は
ターミナルへ移動する必要があった。`Worktree` スコープのヘッダーに
Pull / Push を追加した。

**conflict を UI で扱わない**ことを設計の中心に置いた。Pull は
`git fetch` + `git merge --ff-only` のみで、merge commit の作成・rebase・
conflict 解消は行わない。分岐したブランチや local changes が邪魔をする場合は
Git が処理を中止し、その stderr をエラーとして表示する。ユーザーはターミナルで
解決する。Push も force せず、non-fast-forward はエラーとして返す。

## 変更

### プロトコル / Node

- `WorkspaceGitSyncRequest` / `WorkspaceGitSyncResponse` を追加した。
  応答は branch、HEAD commit、upstream、HEAD が動いたか（pull のみ）を持つ。
- `NodeRpcRequest::WorkspaceGitPull` / `WorkspaceGitPush` と対応する応答を
  追加し、`requires_local_workspace_access()` の対象に含めた。ハブ経由では
  `FORBIDDEN` になる。
- ネットワーク操作のため RPC タイムアウトを 120 秒にした。
- `git::capture_with_timeout` を追加し、fetch / push は 120 秒で打ち切る。
  `kill_on_drop(true)` により、タイムアウト時に子プロセスも終了する。
  `GIT_TERMINAL_PROMPT=0` を設定し、資格情報プロンプトで停止しないようにした。
- `workspace_git_pull` は upstream を確認してから `git fetch` と
  `git merge --ff-only <upstream>` を実行する。upstream がない場合は
  `INVALID_STATE` を返す。
- `workspace_git_push` は upstream があれば `git push`、なければ `origin` が
  存在する場合だけ `git push --set-upstream origin <branch>` を実行する。
  detached HEAD や `origin` がない場合は `INVALID_STATE` を返す。
- 監査ログに `workspace_git_pull` / `workspace_git_push` を追加した。
  path / branch / upstream / commit（pull は updated も）を記録し、
  リモート URL や資格情報は記録しない。

### Client API / UI

- `POST /api/v1/nodes/:node_id/workspace/git/pull` と `.../push` を追加した。
- `Changes` の `Worktree` スコープのヘッダーに ahead / behind 表示と
  Pull / Push のアイコンボタンを追加した。実行中は両方のボタンを無効化し、
  成功時はトースト（pull は更新先の短縮 hash、push は upstream）を出す。
  完了後は `git status` を再取得する。
- 監査ログの表示に `Git pull` / `Git push` を追加した。

## 不変条件

- Pull は fast-forward のみ。merge commit を作らず、conflict marker も作らない。
- Push は force しない。non-fast-forward はエラーとしてユーザーに返す。
- upstream のないブランチの Pull は行わない（何を取得するか決めない）。
- 初回 Push は `origin` がある場合だけ upstream を設定する。それ以外は
  ターミナルでの設定を促す。
- ハブ経由のクライアントは Pull / Push できない（workspace 書き込みは
  ローカル接続限定）。

## 検証

- `cargo test -p fxg-node workspace_git_push_and_pull_are_fast_forward_only`
  で次を確認した。
  - upstream のない初回 push が `origin/<branch>` を設定する。
  - 別 clone の remote change を fast-forward で取り込む。
  - 2 回目の pull は `updated = false`。
  - 分岐した状態の pull / push は `GIT_FAILED` になり、local file と
    HEAD が変わらない（merge commit を作らない）。
- 既存の workspace テストで、upstream / origin のない pull / push が
  `INVALID_STATE` になることを確認した。
- `mise run types` で `ui/src/lib/generated/bindings.ts` を更新した。
- `pnpm --dir ui run check` / `mise run fmt:check` を実行する。
