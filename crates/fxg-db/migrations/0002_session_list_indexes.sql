-- Session list keyset indexes.
--
-- The session list is ordered by (updated_at DESC, session_id ASC) and can be
-- filtered by project / node. Include the tie-breaker in the indexes so SQLite
-- can satisfy both the ORDER BY and the cursor predicate (`updated_at < ?` or
-- `updated_at = ? AND session_id > ?`) from the index, without a TEMP B-TREE
-- sort over the whole table.

-- Global order (no project / node filter).
CREATE INDEX idx_sessions_updated ON sessions(updated_at DESC, session_id ASC);

-- Per-project order. Replaces the old (project_id, updated_at DESC) index.
DROP INDEX idx_sessions_project_updated;
CREATE INDEX idx_sessions_project_updated
    ON sessions(project_id, updated_at DESC, session_id ASC);

-- Per-node order. Replaces the old (node_id, updated_at DESC) index.
DROP INDEX idx_sessions_node_updated;
CREATE INDEX idx_sessions_node_updated
    ON sessions(node_id, updated_at DESC, session_id ASC);
