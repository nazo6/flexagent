# セッション内検索と検索結果のスコープ

- **日付**: 2026-11-25
- **対象パッケージ**: `fxg-db`, `fxg-api`, `ui`, `docs`
- **対象スクリプト**: `mise run fmt`, `cargo test -p fxg-db --test db_layer`,
  `pnpm --dir ui run check`, `pnpm --dir ui run test`

---

## 概要

全文検索は全セッション横断だけで、開いている会話の中を探す手段が無かった
(ブラウザのページ内検索は読み込み済みの DOM にしか効かない)。検索 API に
`session_id` を追加し、ターン目次 (`TurnOutline`) の上部からセッション内を
検索して、ヒットしたイベントへ直接移動できるようにした。

## 変更

### `fxg-db`

- `search` / `search_like` に `session_id: Option<&str>` を追加した。`Some`
  のときは `session_events.session_id` で絞り、`None` は従来どおり全件。
  FTS5 と LIKE フォールバックの両方に適用する。
- セッションを絞ると他セッションのヒットが返らないことをテストに追加した。

### `fxg-api`

- `GET /api/v1/search` に任意の `session_id` クエリを追加し、DB へ渡す。

### `ui`

- `ApiClient.search` に任意の `sessionId` を追加した。
- `TurnOutline` に検索欄を置き、入力から 250ms 後にセッション内検索を実行
  する。結果は投稿順にスニペット付きで並べ、選択すると `onjumpseq` で
  タイムラインの該当イベントへ移動して点滅させる (古いページは必要に応じて
  読み込む)。
- スニペットの `[match]` 分割を `#lib/search.ts` に共通化し、グローバル検索
  ページと共用する。単体テストを追加した。

## 検証

- `cargo test -p fxg-db --test db_layer search`
- `cargo clippy -p fxg-db -p fxg-api --all-targets -- -D warnings`
- `pnpm --dir ui run test` (`search.test.ts` / `turn-outline.test.svelte.ts`)
- `pnpm --dir ui run check`
- 手動: Turns を開いて語句を入力し、ヒットを選ぶと対象行へ移動すること。
  ヒットが無いときの空状態、検索語を消したときに目次へ戻ること。
