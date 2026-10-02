-- sessions 投影に `usage_json` を追加する。
--
-- `UsageUpdated` イベント (ACP `session/update` の `usage_update`) から
-- コンテキスト使用量と累積コストを投影する。NULL = 未受信
-- (docs/02-database-schema.md §0.3 の投影規則を参照)。
ALTER TABLE sessions ADD COLUMN usage_json TEXT; -- SessionUsage (null = 未受信)
