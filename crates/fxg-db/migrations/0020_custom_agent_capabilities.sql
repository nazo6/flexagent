-- ACP client capabilities advertised by a custom ACP agent (node.db).
--
-- Built-in agents are read-only and use the defaults; a custom definition owns
-- its own selection so it can be imported and edited in one place.
ALTER TABLE custom_agents
    ADD COLUMN capabilities_json TEXT NOT NULL DEFAULT '{"subagents":true}';
