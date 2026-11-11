-- Node-owned pairing state (node.db).
--
-- Only the hub address lives here. The pairing token is a secret and stays in
-- `~/.flexagent/node_token` (0600). Both values are owned by the node and edited
-- through the client API
-- (docs/changelog/2026-11-11-node-pairing-db-and-gui.md).

CREATE TABLE node_pairing (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    hub_url     TEXT,
    revision    INTEGER NOT NULL DEFAULT 1,
    updated_at  INTEGER NOT NULL
);
