-- The default agent was never applied (clients choose the first available
-- agent), and the hard-coded "opencode" default is no longer needed.
ALTER TABLE agent_settings DROP COLUMN default_agent;
