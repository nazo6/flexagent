-- Bind Web Push subscriptions to a browser profile so notifications can target the
-- device the user is currently using. Existing rows stay NULL and are backfilled by
-- the next `POST /api/v1/push/subscribe` from that device.
ALTER TABLE push_subscriptions ADD COLUMN client_id TEXT;

CREATE INDEX idx_push_subscriptions_client_id ON push_subscriptions(client_id);
