-- Remove the Git credential proxy configuration.
--
-- Git authentication for ephemeral environments now comes from the
-- provisioner's `env_secrets` (environment variables injected at spawn time).
-- The hub no longer resolves or relays tokens.

DROP TABLE git_credentials;
