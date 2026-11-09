-- Remove the legacy revert / context-fork machinery (2026-11-09).
--
-- `session_reverted` payloads no longer exist in the protocol, so drop the
-- historical rows. Conversation events are kept; only the revert marker
-- disappears (the event has no projection side effects).
DELETE FROM session_events WHERE event_type = 'session_reverted';

-- Fork is now ACP-native and cannot fork at an arbitrary event, so the fork
-- baseline no longer exists. `parent_session_id` keeps the lineage.
ALTER TABLE sessions DROP COLUMN fork_from_node_seq;

-- Cross-node context forks (and the ephemeral VM git bundle handover) are gone.
ALTER TABLE sessions DROP COLUMN git_bundle_path;
