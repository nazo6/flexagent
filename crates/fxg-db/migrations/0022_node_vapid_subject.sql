-- Add Web Push VAPID contact claim to node_settings for standalone single-node operation.
ALTER TABLE node_settings ADD COLUMN vapid_subject TEXT;
