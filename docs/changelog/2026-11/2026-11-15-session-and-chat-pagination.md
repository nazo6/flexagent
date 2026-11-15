# セッション一覧とチャット履歴のページング

- **日付**: 2026-11-15
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-hub`, `fxg-node`, `ui`,
  `docs`
- **対象スクリプト**: `mise run fmt`, `mise run types`,
  `cargo test --workspace`, `pnpm --dir ui run check`, `pnpm --dir ui run test`

セッション一覧が最新200件で打ち切られ、それより古いセッションへ UI から
到達できない問題と、チャット履歴が初回 WS リプレイで全件メモリに載る問題を
まとめて解消した。

## 実装

- **セッション一覧のカーソルページング**: `GET /api/v1/sessions` に
  `cursor` を追加し、応答に `next_cursor` を返す。順序は
  `updated_at` 降順 → `session_id` 昇順のキーセットで、`SessionCursor`
  （`updated_at:session_id`、クライアントには不透明）を次ページに渡す。
  DB 側は `list_session_page` が `has_more` を返し、既存の
  `list_sessions` はその `sessions` を返す薄いラッパーにした。
- **サイドバー**: 最新100件を初期表示し、末尾の「Load older sessions」で
  次ページを追記する。イベント反映時の再取得は先頭ページのみにし、明示的に
  読み込んだ古いページは保持する（更新されたセッションは `updated_at` が
  進むため先頭ページに現れる）。アーカイブは `updated_at` を進めないため、
  `session_archived` で読み込み済みエントリを直接更新する。
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
- `fxg-node`: `/api/v1/sessions?limit=2` → `cursor` で全件を重複なく
  列挙できること、`before_cursor` 応答が `first_cursor` を返すことを検証。
- `ui` (Vitest): サイドバーのカーソル追記と、チャット履歴の
  最新ページ読み込み・`before_cursor` による追加読み込みを検証。
