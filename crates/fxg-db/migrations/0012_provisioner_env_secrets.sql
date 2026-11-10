-- Provisioner secret environment variables.
--
-- The DB stores only the secret ID; the value is read from
-- ~/.flexagent/secrets/hub/<id> when the provisioner child process is spawned.

ALTER TABLE provisioners
    ADD COLUMN env_secrets_json TEXT NOT NULL DEFAULT '{}';
