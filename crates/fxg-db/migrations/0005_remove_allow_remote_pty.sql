-- Drop the remote PTY toggle from node settings.
--
-- Any client that could reach the hub could already enable the setting and
-- then open the PTY, while direct node connections bypassed it entirely. The
-- toggle therefore never acted as a security boundary; access to the hub or
-- node token is the real boundary.

ALTER TABLE node_settings DROP COLUMN allow_remote_pty;
