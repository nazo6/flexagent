# ACPネイティブForkへの一本化とRevert/履歴Fork機構の削除

- **日付**: 2026-11-09
- **対象パッケージ**: `fxg-protocol` / `fxg-db` / `fxg-acp` / `fxg-node` /
  `fxg-server` / `fxg-cli` / `ui`
- **対象スクリプト**: `mise run types`, `mise run sqlx:prepare`,
  `cargo test --workspace`, `pnpm --dir ui run check`, `pnpm --dir ui run test`

## 背景

Revert は ACP にネイティブ機能が無く、Shadow Git Tree のスナップショット復元と
履歴 Replay 注入を組み合わせた fxg 独自実装になっていた。Fork も
`session/fork` が使えない場合に履歴 Replay（64,000 文字上限）へ無言で
フォールバックし、クロスノード / クロスエージェント / 一時VM では Git bundle まで
使う二重実装になっていた。これらは会話状態とファイル状態が一致せず壊れやすいため、
ACP がネイティブに提供する機能だけに絞る。

## 変更内容

### Revert の完全削除

- Shadow Git Tree による毎ターンスナップショット取得・復元
  (`snapshot.rs`, `RestoreOutcome`, `snapshot_tree_hash`) を削除。
- `SessionManager::revert` / `rebuild_session_context` /
  `find_revert_snapshot` を削除。
- `SessionRevertRequest` / `SessionRevertResponse`、
  `POST /api/v1/sessions/:id/revert`、`RevertSession` / `RevertResult`、
  IPC `SessionRevert` / `SessionReverted`、監査 `session_revert`、
  `SNAPSHOT_UNAVAILABLE` / `RevertToNearestSnapshot` を削除。
- CLI `fxg session revert`、Web UI の Revert ボタン・確認ダイアログ・
  タイムラインの巻き戻し表示を削除。
- `UnifiedEventPayload::SessionReverted` と `EventType::SessionReverted` を削除。
  既存 DB の `session_reverted` イベントは migration
  `0008_remove_revert_and_context_fork.sql` で削除する。

### Fork のACPネイティブ一本化

- `SessionManager::fork` は同一ノード上で
  `session.agent_session_id` を指定して `session/fork` + `session/load` を
  呼ぶだけにした。履歴 Replay・`fork_from_node_seq`・別エージェント指定を削除。
- `NativeForkUnavailable` は `NodeError::ForkUnsupported` /
  `ErrorCode::FORK_UNSUPPORTED` として返し、履歴 Replay へフォールバックしない。
- サーバー経由の Fork はソースセッションのノードにのみルーティングし、
  `StartSession.fork_from_session_id` で伝達する。provisioner・別ノード・
  一時VMの Fork は明示エラー。
- `CapabilitiesUpdated.fork_supported` を追加し、ACP の
  `sessionCapabilities.fork` と `loadSession` の両方が揃った場合だけ true に
  する。`sessions.fork_supported` へ投影し、Web UI は非対応時に Fork ボタンを
  表示しない（migration `0009_sessions_fork_supported.sql`）。
- CLI の `fxg session fork` から `--from-seq` / `--agent` / `--provisioner` を
  削除（`-w/--worktree` は残す）。UI のメッセージ単位 Fork を削除し、
  セッション単位 Fork のみ残す。

### 一時VM Git bundle の削除

- `bundle.rs`、`WorkspaceBundleUpload`、`sessions.git_bundle_path`、
  provisioner の bundle 蓄積・保存・復元処理、`create_git_bundle` を削除。
- `DrainAndShutdown` はイベントフラッシュと `DrainComplete` のみ行う。
- `git_available()` は `fxg-node::git` へ移動。

### Resume は現状維持

Resume は「ネイティブ復元を優先し、明示操作時のみ履歴 Replay」を維持する。
共有ヘルパー `build_history_replay_context` は Resume 専用に簡素化した。

## 互換性

- `UnifiedEventPayload::SessionReverted` の削除に伴い、既存の `session_reverted`
  イベントは migration で削除する（イベント投影への副作用は無い）。
- `sessions.fork_from_node_seq` / `sessions.git_bundle_path` カラムを削除。
  `parent_session_id` は Fork 系譜として残す。
- 旧 `UserMessage.snapshot_tree_hash` / `SessionCreated.fork_from_node_seq` は
  未知フィールドとして無視される。
- `.sqlx` と `ui/src/lib/generated/bindings.ts` を再生成済み。

## 検証

- `mise run sqlx:prepare`
- `mise run types`
- `cargo test -p fxg-protocol -p fxg-db -p fxg-acp -p fxg-node -p fxg-server -p fxg-cli`
- `pnpm --dir ui run check`
- `pnpm --dir ui run test`
