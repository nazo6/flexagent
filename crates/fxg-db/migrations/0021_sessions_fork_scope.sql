-- Replace the boolean fork capability with the fork scope reported by the agent.
--
-- The ACP-native fork path can only fork the current end of the conversation,
-- so existing `fork_supported = 1` sessions map to `last`. `message` is reserved
-- for agents that expose a conversation boundary in a future extension.
ALTER TABLE sessions ADD COLUMN fork_scope TEXT NOT NULL DEFAULT 'none';
UPDATE sessions SET fork_scope = CASE WHEN fork_supported <> 0 THEN 'last' ELSE 'none' END;
ALTER TABLE sessions DROP COLUMN fork_supported;
