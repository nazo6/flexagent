-- Registry imports are stored as custom agents with display metadata and
-- provenance. Package enablement is replaced by importing an npx/uvx recipe.
ALTER TABLE custom_agents ADD COLUMN description TEXT;
ALTER TABLE custom_agents ADD COLUMN icon TEXT;
ALTER TABLE custom_agents ADD COLUMN origin_json TEXT;

DROP TABLE IF EXISTS enabled_package_agents;
