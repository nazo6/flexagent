# 外部セッション一覧の刷新とドリフト検出

- **日付**: 2026-11-25
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-node`, `fxg-hub`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run sqlx:prepare`, `mise run fmt`,
  `cargo test -p fxg-protocol -p fxg-db -p fxg-node -p fxg-hub -p fxg-cli`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `pnpm --dir ui run check`,
  `pnpm --dir ui run test`, `pnpm --dir ui run lint`

---

## 概要

外部セッションの取り込みが「一覧を開く → スキャン完了を待つ → 行ごとに
Import」という流れで重く、またエージェント側の一覧には fxg で開始した
セッションも混ざるため見分けづらかった。UI を専用ページに移し、未取り込みを
既定タブにして、行を開くだけで取り込めるようにした。さらに、取り込み後に
外部クライアントで進んだ会話を検出して明示的に再取り込みできるようにした。

## 変更

### `fxg-protocol`

- `ExternalSessionSummary` に `imported_title` / `imported_updated_at` /
  `imported_status` / `needs_sync` を追加した。`needs_sync` はエージェントの
  `updated_at` と fxg セッションの最終更新を比較するベストエフォート値。
- `ExternalSessionListResponse.scanned_at` を追加し、キャッシュされた結果でも
  実際のスキャン時刻を表示できるようにした。
- `SessionImportRequest.replace_session_id` を追加した。指定時は現在の
  エージェント状態を新しい fxg セッションへ取り込み、成功後に旧セッションを
  アーカイブする。
- `NodeRpcRequest::ImportSession` に `replace_session_id` を追加（ハブ経由の
  取り込みでも同じ動作）。

### `fxg-db`

- `list_agent_session_bindings` が `AgentSessionBinding`
  (`title` / `status` / `updated_at` を含む) を返すようにした。削除済みと
  アーカイブ済みは除外する。
- `find_session_by_agent_session` はアーカイブ済みを無視する。再取り込みで
  古いアーカイブが新しい取り込みを妨げない。

### `fxg-node`

- 外部セッションの発見を最大 3 エージェント並列にした。プローブとの排他は
  従来どおり `probe_lock` で維持する。
- 取り込みを冪等化した。同じ `(agent_id, agent_session_id)` に生きている
  非アーカイブの fxg セッションがあれば、その ID を返して二重束縛しない。
  取り込みはノード内で直列化する。
- `replace_session_id` による再取り込みを実装した。旧セッションを停止・
  アーカイブしてから新しいセッションを作り、失敗時は旧セッションを
  アンアーカイブする。
- 取り込み監査ログに `replaced_session_id` を記録する。

### `ui`

- `ExternalSessionsDialog` を廃止し、`/external` ページを追加した。
  - 「未取り込み / 取り込み済み / すべて」タブ（既定は未取り込み）。
  - タイトル・フォルダ・エージェントの検索、エージェントフィルタ、
    プロジェクト別グルーピング、最終スキャン時刻。
  - 行の「開く」で未取り込みなら取り込み、取り込み済みなら既存セッションへ。
  - 取り込み済みの行には「Refresh from agent」があり、`needs_sync` の
    ときはバッジとラベルを出して目立たせる。現在のエージェント状態を
    新しいセッションとして取り込む（確認ダイアログ付き）。
  - 検出できなかったエージェントは折りたたみの「Detection issues」に集約。
- サイドバーの外部セッション項目をページへのリンクに変更した。
- キャッシュ済みのスキャンは即表示し、15 秒以上古い場合はバックグラウンドで
  再スキャンする。

## 制約

- `needs_sync` は時刻比較のヒューリスティックである。クロック差を吸収する
  ため 60 秒の猶予を置くが、同時刻付近の外部更新は取りこぼす可能性がある。
- 再同期は新しい fxg セッション ID を発行し、旧セッションをアーカイブする。
  ACP replay に安定したメッセージ ID が無いため、履歴の差分マージはしない。
- ACP は外部クライアントの追記をライブ同期できない。OpenCode native は
  同じ `opencode serve` に接続している間だけ SSE で外部操作を拾う。
- 外部セッションを開くとエージェントプロセスがアイドル状態で残る
  （既存の取り込み挙動）。

## 検証

- `cargo test -p fxg-node import_session_`:
  冪等取り込み、再取り込みのアーカイブ、失敗時のアンアーカイブ。
- `cargo test -p fxg-node list_external_sessions_marks_already_imported_sessions`:
  取り込み済みメタデータと `needs_sync` / `scanned_at`。
- `cargo clippy --workspace --all-targets -- -D warnings`
- `pnpm --dir ui run check` / `pnpm --dir ui run test` / `pnpm --dir ui run lint`
