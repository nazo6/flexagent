# セッション一覧とチャット履歴のページング

- **日付**: 2026-11-15（2026-11-16 追記）
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-hub`, `fxg-node`, `fxg-cli`,
  `ui`, `docs`
- **対象スクリプト**: `mise run fmt`, `mise run fmt:check`,
  `mise run lint:errors`, `mise run sqlx:prepare`, `mise run sqlx:check`,
  `mise run types`, `cargo test --workspace`, `pnpm --dir ui run check`,
  `pnpm --dir ui run test`

セッション一覧が最新200件で打ち切られ、それより古いセッションへ UI から
到達できない問題と、チャット履歴が初回 WS リプレイで全件メモリに載る問題を
まとめて解消した。

## 実装

- **セッション一覧のカーソルページング**: `GET /api/v1/sessions` に
  `cursor` を追加し、応答に `next_cursor` を返す。順序は
  `updated_at` 降順 → `session_id` 昇順のキーセットで、`SessionCursor`
  （`updated_at:session_id`、クライアントには不透明）を次ページに渡す。
  応答の `project_cursors` はページに含まれるプロジェクトごとの続き
  カーソルで、プロジェクト単位の追加読み込みに使う。DB 側は
  `list_session_page` が `has_more` と `project_cursors` を返し、既存の
  `list_sessions` はその `sessions` を返す薄いラッパーにした。
- **セッション一覧の SQL 押し下げ（2026-11-16 追記）**:
  `list_session_page` が全セッションを取得して Rust でフィルタしていたため、
  UI の通常表示・イベント更新・トレイ・起動処理で毎回 O(全セッション) の
  スキャンが走っていた。`QueryBuilder` で `project_id` / `node_id` /
  `status` / `include_archived` / `cursor` を `WHERE` に組み立て、
  `ORDER BY updated_at DESC, session_id ASC` と `LIMIT limit + 1` を SQL で
  適用する。`has_more` は追加取得した 1 件で判定する。行の型は
  `SessionRow`（`FromRow`）のまま維持し、モジュール冒頭の SQL ポリシーに
  `QueryBuilder` の例外理由を明記した。
  - `project_cursors` はページ内の各プロジェクトについて、ページ末尾を
    カーソルにしたインデックス済み `EXISTS` を 1 回ずつ実行して正確に
    求める。`next_cursor = null` は全件読み込み済み、エントリなしは
    「このページに含まれない（続きは不明）」という既存の意味を維持する。
  - `(updated_at DESC, session_id ASC)` を追加し、既存の
    `idx_sessions_project_updated` / `idx_sessions_node_updated` を
    `(project_id | node_id, updated_at DESC, session_id ASC)` に拡張した。
    `EXPLAIN QUERY PLAN` で `TEMP B-TREE` が消え、プロジェクト絞り込みが
    `idx_sessions_project_updated` の `SEARCH` になることを確認した。
  - 内部の全件取得を軽量化: トレイは `count_sessions`、起動時 reconcile /
    kill switch / ハブの `RunningTurns` は `list_session_ids`、`NodeHello`
    は `list_session_sync_states`（`session_id` / `project_id` /
    `last_node_seq` のみ）、workspace 検証・project link 再解決は
    `list_session_local_paths`、外部セッション取り込みは
    `list_agent_session_bindings` を使う。
  - debug ビルド、1プロジェクト、50k セッション、100件ページの計測:
    `GET /api/v1/sessions?limit=100` は 733ms → 5–20ms、
    `project_id` 絞り込みも 5–20ms（DB 全件取得は 133ms）。
- **サイドバー**: 最新100件を初期表示し、各プロジェクトの
  「Load older sessions」でそのプロジェクトの次ページだけを追記する。
  セッションがまだ読み込まれていないプロジェクトは「Load sessions」で
  先頭ページを取得する。イベント反映時の再取得は先頭ページのみにし、
  明示的に読み込んだ古いページは保持する（更新されたセッションは
  `updated_at` が進むため先頭ページに現れる）。アーカイブは `updated_at`
  を進めないため、`session_archived` で読み込み済みエントリを直接更新する。
- **チャット履歴の上方ページング**: `SessionEventBatch` に `first_cursor`
  を追加し、`before_cursor` に渡す次のカーソルを得られるようにした。
  `GET /api/v1/sessions/:id/events` に `tail=true`（最新ページ）を追加し、
  セッションを開くと最新ページを読み込む。上部の「Load earlier messages」で
  古いページを先頭に追加する（スクロール位置を維持）。セッション先頭の
  ページは capabilities / provisioning log などのメタデータ用に別保持し、
  ページングが先頭へ到達した時点で会話へ統合する。
- **初回リプレイの抑制とメモリ上限**: `GET /api/v1/system/info` に
  `latest_event_cursor` を追加し、保存済みカーソルがないクライアントの初回
  WS 購読はストア末尾から開始する。チャット履歴は REST ページングで取得し、
  保存済みカーソルがある再接続は差分をリプレイする。開いていない
  セッションのタイムラインは 200 イベントを超えた分を古い順に破棄し、
  開いたときに再取得する。

## テスト

- `fxg-db`: 3セッションを2ページに分けて取得し、`next_cursor` の
  往復・重複なし・最終ページの `has_more=false` を検証。
- `fxg-db`: ページがちょうど `limit` 件のとき `has_more=false` になること、
  ページ内で全件読み込み済みのプロジェクトが `next_cursor=null` になり
  グローバルの `has_more` と独立していることを検証。軽量クエリ
  (`count_sessions` / `list_session_ids` / `list_session_sync_states` /
  `list_session_local_paths` / `list_agent_session_bindings`) が一覧と
  一致することを検証。
- `fxg-node`: `/api/v1/sessions?limit=2` → `cursor` で全件を重複なく
  列挙できること、最初のページが `next_cursor` を返すこと、
  `before_cursor` 応答が `first_cursor` を返すことを検証。
- `ui` (Vitest): サイドバーのカーソル追記と、チャット履歴の
  最新ページ読み込み・`before_cursor` による追加読み込みを検証。
- ブラウザ: 600セッション / 5プロジェクトの隔離環境で、プロジェクトごとの
  「Load sessions」「Load older sessions」と、WS イベント後の再取得で
  読み込み済みページが巻き戻らないことを確認。
