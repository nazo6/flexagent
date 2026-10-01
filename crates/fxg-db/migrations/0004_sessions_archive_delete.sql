-- セッションのアーカイブ / 削除 (tombstone) 対応。
--
-- - `archived_at`: アーカイブ日時 (NULL = 未アーカイブ)。
--   アーカイブ済みセッションは一覧の既定から除外される (`include_archived` で包含)。
--   イベントログ・会話内容は保持され、`SessionArchived { archived: false }` で復元できる。
-- - `deleted_at`: 削除 (tombstone) 日時 (NULL = 有効)。
--   適用時にイベント本文・承認履歴をパージする (復元不能)。行自体は
--   Outbox / Resync で削除を伝播し、Fork 元参照の整合を保つため残す
--   (設計: docs/02-database-schema.md §1)。
ALTER TABLE sessions ADD COLUMN archived_at INTEGER;
ALTER TABLE sessions ADD COLUMN deleted_at INTEGER;
