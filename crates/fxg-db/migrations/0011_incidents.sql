-- Diagnostic incidents: a bounded, grouped history of failures and degradations.
--
-- Both node.db and hub.db use this table. Rows are grouped by
-- (code, source, session_id, node_id) while unresolved and inside the grouping
-- window, so a reconnect loop does not flood the table. Retention is enforced by
-- `fxg_db::incidents::prune` (period + maximum row count).

CREATE TABLE incidents (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at        INTEGER NOT NULL,
    last_seen_at      INTEGER NOT NULL,
    level             TEXT NOT NULL,
    code              TEXT NOT NULL,
    summary           TEXT NOT NULL,
    details_json      TEXT NOT NULL DEFAULT '{}',
    session_id        TEXT,
    node_id           TEXT,
    request_id        TEXT,
    source            TEXT NOT NULL,
    occurrence_count  INTEGER NOT NULL DEFAULT 1,
    resolved_at       INTEGER
);

CREATE INDEX idx_incidents_created ON incidents(created_at DESC);
CREATE INDEX idx_incidents_group
    ON incidents(code, source, session_id, node_id, resolved_at);
CREATE INDEX idx_incidents_last_seen ON incidents(last_seen_at DESC);
