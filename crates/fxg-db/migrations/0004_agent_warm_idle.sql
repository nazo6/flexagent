-- Idle retention of warm ACP agent processes.
--
-- This used to be the process environment variable
-- FXG_AGENT_WARM_IDLE_SECS, which the settings API and the UI could not see.
-- Runtime agent tuning is owned by `agent_settings`:
-- NULL = the built-in default, 0 = keep idle processes until node shutdown.

ALTER TABLE agent_settings ADD COLUMN warm_idle_secs INTEGER;
