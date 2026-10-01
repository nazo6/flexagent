-- sessions 投影に `available_modes_json` を追加する。
--
-- `CapabilitiesUpdated` は `current_mode` / `available_modes` /
-- `available_commands` / `config_options` を持つが、`available_modes` だけが
-- 投影カラムを持たず、イベントログ全量を走査しないと復元できなかった
-- (docs/02-database-schema.md §0.3 の投影規則を参照)。
ALTER TABLE sessions ADD COLUMN available_modes_json TEXT NOT NULL DEFAULT '[]'; -- ACP ModeInfo[]
