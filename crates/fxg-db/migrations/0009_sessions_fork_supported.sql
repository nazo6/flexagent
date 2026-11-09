-- ACP-native fork capability reported by `CapabilitiesUpdated`.
-- Old rows default to 0 (the UI hides the Fork action until the agent reports it).
ALTER TABLE sessions ADD COLUMN fork_supported INTEGER NOT NULL DEFAULT 0;
