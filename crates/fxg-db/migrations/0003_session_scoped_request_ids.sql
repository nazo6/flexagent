-- Scope permission / elicitation IDs to their session.
--
-- ACP request IDs are only unique within one agent process / session. Two
-- sessions can legitimately use the same JSON-RPC id (for example "1" or "2"),
-- so a global primary key made one session's approval overwrite another's.
--
-- SQLite cannot change a primary key in place; rebuild both projection tables
-- with a composite key. The old global key guarantees there are no duplicates.

CREATE TABLE permission_requests_new (
    request_id              TEXT NOT NULL,                  -- ACP request id (unique per session)
    session_id              TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_id                 TEXT NOT NULL REFERENCES nodes(node_id),
    tool_name               TEXT NOT NULL,
    summary                 TEXT NOT NULL,
    details_json            TEXT NOT NULL,
    status                  TEXT NOT NULL DEFAULT 'pending',
    resolved_by             TEXT,
    created_at              INTEGER NOT NULL,
    resolved_at             INTEGER,
    PRIMARY KEY (session_id, request_id)
);

INSERT INTO permission_requests_new
    (request_id, session_id, node_id, tool_name, summary, details_json,
     status, resolved_by, created_at, resolved_at)
SELECT request_id, session_id, node_id, tool_name, summary, details_json,
       status, resolved_by, created_at, resolved_at
  FROM permission_requests;

DROP TABLE permission_requests;
ALTER TABLE permission_requests_new RENAME TO permission_requests;
CREATE INDEX idx_permission_pending ON permission_requests(status) WHERE status = 'pending';

CREATE TABLE elicitation_requests_new (
    elicitation_id          TEXT NOT NULL,                  -- ACP elicitation id (unique per session)
    session_id              TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_id                 TEXT NOT NULL REFERENCES nodes(node_id),
    message                 TEXT NOT NULL,
    mode                    TEXT NOT NULL DEFAULT 'form',
    schema_json             TEXT NOT NULL DEFAULT '{}',
    tool_call_id            TEXT,
    status                  TEXT NOT NULL DEFAULT 'pending',
    content_json            TEXT NOT NULL DEFAULT 'null',
    resolved_by             TEXT,
    created_at              INTEGER NOT NULL,
    resolved_at             INTEGER,
    PRIMARY KEY (session_id, elicitation_id)
);

INSERT INTO elicitation_requests_new
    (elicitation_id, session_id, node_id, message, mode, schema_json,
     tool_call_id, status, content_json, resolved_by, created_at, resolved_at)
SELECT elicitation_id, session_id, node_id, message, mode, schema_json,
       tool_call_id, status, content_json, resolved_by, created_at, resolved_at
  FROM elicitation_requests;

DROP TABLE elicitation_requests;
ALTER TABLE elicitation_requests_new RENAME TO elicitation_requests;
CREATE INDEX idx_elicitation_pending ON elicitation_requests(status) WHERE status = 'pending';
