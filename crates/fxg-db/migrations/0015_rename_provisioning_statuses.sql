-- Fold the ephemeral bootstrap status into provisioning and rename the
-- provisioning log event.
--
-- Stored `payload_json` bodies are read through the `bootstrap_log` serde
-- alias; only the indexed event type is rewritten here.

UPDATE sessions
   SET status = 'provisioning'
 WHERE status = 'bootstrapping';

UPDATE nodes
   SET lifecycle_status = 'provisioning'
 WHERE lifecycle_status = 'bootstrapping';

UPDATE session_events
   SET event_type = 'provisioning_log'
 WHERE event_type = 'bootstrap_log';
