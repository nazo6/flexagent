# DB マイグレーションの squash とレガシーフォールバック撤去

- **日付**: 2026-11-15
- **対象パッケージ**: `fxg-db`, `fxg-hub`, `docs`
- **対象スクリプト**: `mise run sqlx:prepare`, `cargo test -p fxg-db`,
  `cargo clippy -p fxg-db -p fxg-hub --all-targets`

---

## 概要

プレリリース期間中に積み上がった 22 本のマイグレーション
（`0001`〜`0022`）を、最新スキーマを直接作成する `0001_initial.sql` 1 本へ
squash した。旧マイグレーションが持っていた列追加・列削除・テーブル削除・
値を書き換える `UPDATE` はすべて不要になり、初期状態から現在のスキーマが
そのまま得られる。

これに伴い、旧 DB の内容だけを想定していたフォールバックを削除した。

## 変更

### `fxg-db`: マイグレーション

- `crates/fxg-db/migrations/0001_initial.sql` を最新スキーマで書き直した
  （コメントは英語、`docs/development.md` の方針どおり）。
- `0002`〜`0022` を削除した。
- 実効スキーマが squash 前と同一であること（テーブル・列・デフォルト値・
  インデックス・FTS5 トリガー）を sqlite の `pragma table_info` と
  `sqlite_master` で比較して確認した。
- 旧 DB は `_sqlx_migrations` のチェックサムが一致しないため起動できない。
  開発環境では `fxg internal wipe` でデータディレクトリを作り直す。本番未
  デプロイのため移行コマンドは用意しない。

### レガシーフォールバックの削除

- `queries::resolve_default_local_path` は、紐付けが無い場合に
  「最新セッションの `local_path`」へフォールバックする処理を削除した。
  セッション開始時に必ず `project_node_bindings` を登録する現在の設計では、
  バインドだけが既定パスの情報源になる。
- `events::resolve_permission_status` は、`details_json` を復号できない場合に
  `option_id` の命名（`reject` / `deny` を含むか）から推測するフォールバックを
  削除した。`PermissionResolved` に対応する `permission_requests` 行が無い
  場合は何も更新せず、選択肢が保存内容に存在しない場合は
  `DbError::InvalidPayload` を返す。

## 検証

- `cargo test -p fxg-db` でマイグレーション適用・投影再構築・権限解決の
  テストが通ること。
- `cargo clippy -p fxg-db -p fxg-hub --all-targets -- -D warnings` が通ること。
- `mise run sqlx:prepare` で `.sqlx` を再生成し、オフラインキャッシュが
  最新スキーマと一致すること。
