-- Remove the per-project bootstrap profile.
--
-- Toolchain setup and workspace preparation are now owned by the container
-- image / VM template / provisioner launch command.

DROP TABLE project_profiles;
