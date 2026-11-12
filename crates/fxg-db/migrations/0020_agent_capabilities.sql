-- Per-agent ACP client capability overrides (node.db).
--
-- Only agents that differ from the defaults get a row, so the table stays
-- small and a new default applies immediately to every other agent.
CREATE TABLE agent_capabilities (
    agent_id   TEXT PRIMARY KEY NOT NULL,
    subagents  INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
