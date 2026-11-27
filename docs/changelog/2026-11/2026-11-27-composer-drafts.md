# 複数コンポーザー下書きと端末間共有

- **日付**: 2026-11-27
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-api`, `fxg-node`, `fxg-hub`, `ui`
- **対象スクリプト / 設定**:
  - `crates/fxg-db/migrations/0006_drafts.sql`（新規）
  - `mise run types`
  - `mise run sqlx:prepare`
  - `cargo test -p fxg-db --test drafts`
  - `cargo test -p fxg-node rest_drafts_crud_and_revision_conflict`
  - `pnpm --dir ui run check`
  - `pnpm --dir ui run test`
  - `pnpm --dir ui run build`
  - `mise run fmt:check`

## 背景

下書きはセッションごとに 1 件だけ `localStorage` に保存しており、添付はメモリのみ、
端末をまたげなかった。複数候補のプロンプトを保持し、ハブ / ノードに接続する
端末間で共有するため、下書きを独立したエンティティにした。

設計: [`docs/proposals/2026-11-27-composer-drafts-and-sync.md`](../../proposals/2026-11-27-composer-drafts-and-sync.md)

## 変更

- **DB**:
  - `drafts` テーブルを追加した（`draft_id`, `title`, `body`, `session_id`,
    `project_id`, `node_id`, `attachments_json`, `revision`, `origin_device_id`,
    `created_at`, `updated_at`, `deleted_at`）。
  - 更新は `revision` の楽観ロック、削除は tombstone。`session_id` に外部キーは張らない。
- **プロトコル**:
  - `DraftId` / `Draft` / `DraftListResponse` / `DraftListQuery` /
    `CreateDraftRequest` / `UpdateDraftRequest` を追加した。
  - `ServerWsMessage::DraftChanged { draft_id, revision, deleted }` を追加した。
- **API**:
  - `GET/POST /api/v1/drafts` と `GET/PATCH/DELETE /api/v1/drafts/:id` を
    ハブ / ノード共通のルータに追加した。
  - `expected_revision` の不一致は `409 REVISION_MISMATCH` を返す。
  - 作成・更新・削除を Client WS の `DraftChanged` で接続中のクライアントへ通知する。
- **UI**:
  - コンポーザーに Drafts メニューを追加し、切替・新規・リネーム・削除を可能にした。
  - サイドバーの Drafts ビューでセッションなし下書きを一覧し、開く・リネーム・削除できる。
  - 本文は 800ms のデバウンスで自動保存する。保存中 / 未保存を表示し、失敗時は再試行できる。
  - 送信成功時はその下書きを削除して新しい空の下書きを用意する。
  - 旧 `localStorage` の下書きは初回ロード時にサーバーの下書きへ移行する。
  - 競合時はサーバー側の内容を採用し、手元の本文を複製として残す。
  - 他端末の `DraftChanged` を反映する。編集中（未保存の変更あり）の端末は上書きしない。
  - 選択中の下書き ID は端末ローカルに保持する。
  - 画面離脱時とタブ非表示時に保留中の保存をフラッシュする。

## 不変条件

- 下書きは `session_events` / Outbox に載せない。接続先ストアがそのスコープの権威。
- 下書きの更新は `revision` が一致した場合だけ成功する。
- `session_id` はソフト参照。セッション削除後も下書きは残る。
- v1 は添付の実体を同期しない。メタデータのみで、実体は端末に残る。
- v1 は node ⇔ hub の下書き複製と Client WS の変更通知を持たない。
  他端末の更新は再読み込み / 再マウント時に反映する。

## 検証

- `cargo test -p fxg-db --test drafts`
- `cargo test -p fxg-node rest_drafts_crud_and_revision_conflict`
- `cargo clippy -p fxg-protocol -p fxg-db -p fxg-api -p fxg-node -p fxg-hub --all-targets`
- `pnpm --dir ui run check`
- `pnpm --dir ui run test`（285 件）
- `pnpm --dir ui run build`
- `mise run fmt:check`

## 残タスク

[`docs/backlog.md`](../../backlog.md) §8 を参照。リアルタイム通知、添付共有、
node ⇔ hub 複製、サイドバーの Drafts ビュー、tombstone の GC が未対応。
