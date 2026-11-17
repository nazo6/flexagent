-- FlexAgent single schema (shared by node.db and hub.db).
--
-- Design: docs/02-database-schema.md §1
--   node.db (<data>/node/node.db) and hub.db (<data>/hub/hub.db) are generated
--   from this one schema. Their only differences are row scope and execution
--   role, so schema symmetry is guaranteed by a single definition.
--
--   PRAGMA journal_mode = WAL / PRAGMA foreign_keys = ON have no effect inside a
--   migration transaction; they are applied to every connection through
--   SqliteConnectOptions in crates/fxg-db/src/db.rs.

-- 1. Node registry.
--    Node: only the node's own row (for FK references and the local UI).
--    Hub: every node (hub-authoritative data updated by NodeHello; token_hash
--    is used only by the hub).
CREATE TABLE nodes (
    node_id                 TEXT PRIMARY KEY,               -- UUID or fixed slug (e.g. "win-desktop", "eph-0195...")
    name                    TEXT NOT NULL,                  -- Display name (e.g. "Home Windows PC", "Colab Pro (T4)")
    os                      TEXT NOT NULL,                  -- "windows" | "linux" | "macos"
    arch                    TEXT NOT NULL,                  -- "x86_64" | "aarch64"
    version                 TEXT NOT NULL,                  -- fxg binary version
    token_hash              TEXT,                           -- Node token hash (SHA-256); NULL = not issued
    token_issued_at         INTEGER,                        -- Last token issue time (Unix epoch ms, optional)
    installed_agents_json   TEXT NOT NULL DEFAULT '[]',     -- Installed agent IDs (JSON array)
    is_ephemeral            INTEGER NOT NULL DEFAULT 0,     -- 0: resident node, 1: ephemeral VM/container node
    provisioner             TEXT,                           -- Provisioner identifier of an ephemeral node
    lifecycle_status        TEXT NOT NULL DEFAULT 'ready',  -- 'provisioning' | 'ready' | 'draining' | 'terminated' | 'error'
    idle_timeout_secs       INTEGER,                        -- Idle time before automatic discard (ephemeral nodes)
    is_online               INTEGER NOT NULL DEFAULT 0,     -- 1: connected, 0: disconnected
    last_seen_at            INTEGER NOT NULL,               -- Unix epoch (ms)
    created_at              INTEGER NOT NULL,
    state_revision          INTEGER NOT NULL DEFAULT 0      -- Bumped when the hub's view of the node changes
);

-- 2. Logical projects.
--    Node: only the projects it participates in. Hub: the aggregate over all
--    nodes (hub-authoritative).
CREATE TABLE projects (
    project_id              TEXT PRIMARY KEY,               -- Normalized key (e.g. "github.com/nazo6/flexagent")
    name                    TEXT NOT NULL,                  -- Display name (e.g. "flexagent")
    canonical_git_url       TEXT,                           -- Normalized source Git URL
    identity_kind           TEXT NOT NULL DEFAULT 'local',  -- 'git_remote' | 'user_key' | 'local'
    project_key             TEXT,                           -- Explicit project key for path links
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL
);

-- 3. Project × node local path bindings (including worktrees).
CREATE TABLE project_node_bindings (
    project_id              TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    node_id                 TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    local_path              TEXT NOT NULL,                  -- e.g. "D:\ghq\github.com\nazo6\flexagent" or a worktree path
    is_worktree             INTEGER NOT NULL DEFAULT 0,     -- 1: Git worktree, 0: main repository
    path_exists             INTEGER NOT NULL DEFAULT 1,     -- Whether the path existed when the node last reported it
    git_branch              TEXT,                           -- Git branch at the last check
    last_used_at            INTEGER NOT NULL,
    PRIMARY KEY (project_id, node_id, local_path)
);

-- 4. Sessions.
--    Node: its own sessions (authoritative; written directly).
--    Hub: the projection of all nodes (updated only in the same transaction as
--    event application).
CREATE TABLE sessions (
    session_id              TEXT PRIMARY KEY,               -- UUID v7
    project_id              TEXT NOT NULL REFERENCES projects(project_id),
    node_id                 TEXT NOT NULL REFERENCES nodes(node_id),
    local_path              TEXT NOT NULL,                  -- Execution directory (including a worktree path)
    git_branch              TEXT,                           -- Snapshot at start-up (live value comes from the worktree API)
    is_worktree             INTEGER NOT NULL DEFAULT 0,     -- Whether it runs inside a worktree
    agent_id                TEXT NOT NULL,                  -- e.g. "opencode" | "agy"
    agent_session_id        TEXT,                           -- Agent-internal session ID (set by SessionAgentBound)
    parent_session_id       TEXT REFERENCES sessions(session_id), -- Session this one was forked from
    title                   TEXT NOT NULL DEFAULT 'New Session',
    status                  TEXT NOT NULL,                  -- 'provisioning' | 'idle' | 'running' | 'waiting_permission' | 'waiting_input' | 'stopped' | 'error'
    current_mode            TEXT,                           -- ACP SessionMode (e.g. 'code', 'plan')
    available_modes_json    TEXT NOT NULL DEFAULT '[]',     -- ACP ModeInfo[]
    available_commands_json TEXT NOT NULL DEFAULT '[]',     -- ACP AvailableCommand[]
    config_options_json     TEXT NOT NULL DEFAULT '[]',     -- ACP ConfigOption[]
    usage_json              TEXT,                           -- SessionUsage (NULL = not received)
    fork_scope              TEXT NOT NULL DEFAULT 'none',   -- 'none' | 'last' | 'message'
    permission_policy       TEXT NOT NULL DEFAULT 'ask',    -- 'ask' | 'auto_allow_all'
    archived_at             INTEGER,                        -- NULL = not archived
    deleted_at              INTEGER,                        -- NULL = live; otherwise the deletion tombstone time
    last_node_seq           INTEGER NOT NULL DEFAULT 0,     -- Node: latest persisted node_seq / hub: latest applied node_seq
    synced_up_to_node_seq   INTEGER NOT NULL DEFAULT 0,     -- Node only: watermark acknowledged by the hub (hub keeps 0)
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL
);

CREATE INDEX idx_sessions_project_updated ON sessions(project_id, updated_at DESC);
CREATE INDEX idx_sessions_status ON sessions(status);
CREATE INDEX idx_sessions_node_updated ON sessions(node_id, updated_at DESC);

-- 5. Session event history (append-only event log).
--    Node: events of its own sessions. Hub: events of every node (idempotent
--    append).
CREATE TABLE session_events (
    cursor                  INTEGER PRIMARY KEY AUTOINCREMENT, -- Ingestion order in this DB (client resume cursor)
    event_id                TEXT NOT NULL UNIQUE,           -- UUID v7 (idempotent application and deduplication)
    session_id              TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_seq                INTEGER NOT NULL,               -- Order within the session (1, 2, 3...). Only the running node assigns it
    event_type              TEXT NOT NULL,                  -- 'session_created' | ... | 'status_changed'
    payload_json            TEXT NOT NULL,                  -- Structured payload (UnifiedEventPayload JSON; the source of truth)
    searchable_text         TEXT,                           -- Plain text for FTS5 (generated from payload_json; binary events excluded)
    created_at              INTEGER NOT NULL,
    UNIQUE(session_id, node_seq)
);

CREATE INDEX idx_session_events_session_seq ON session_events(session_id, node_seq ASC);

-- 6. FTS5 full-text search index (conversation, thoughts, file paths and
--    command history).
CREATE VIRTUAL TABLE session_events_fts USING fts5(
    session_id UNINDEXED,
    event_type UNINDEXED,
    searchable_text,
    content='session_events',
    content_rowid='cursor',
    tokenize='trigram'                                  -- Handles Japanese and code identifiers without a morphological analyzer
);

-- FTS5 external-content synchronization triggers (INSERT / DELETE / UPDATE).
CREATE TRIGGER session_events_fts_ai AFTER INSERT ON session_events BEGIN
    INSERT INTO session_events_fts(rowid, session_id, event_type, searchable_text)
    VALUES (new.cursor, new.session_id, new.event_type, new.searchable_text);
END;
CREATE TRIGGER session_events_fts_ad AFTER DELETE ON session_events BEGIN
    INSERT INTO session_events_fts(session_events_fts, rowid, session_id, event_type, searchable_text)
    VALUES ('delete', old.cursor, old.session_id, old.event_type, old.searchable_text);
END;
CREATE TRIGGER session_events_fts_au AFTER UPDATE ON session_events BEGIN
    INSERT INTO session_events_fts(session_events_fts, rowid, session_id, event_type, searchable_text)
    VALUES ('delete', old.cursor, old.session_id, old.event_type, old.searchable_text);
    INSERT INTO session_events_fts(rowid, session_id, event_type, searchable_text)
    VALUES (new.cursor, new.session_id, new.event_type, new.searchable_text);
END;

-- 7. Approval inbox (projection applied from events; fast lookup of unresolved
--    permission requests).
CREATE TABLE permission_requests (
    request_id              TEXT PRIMARY KEY,               -- ACP request id
    session_id              TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_id                 TEXT NOT NULL REFERENCES nodes(node_id), -- The node itself on the node side
    tool_name               TEXT NOT NULL,                  -- e.g. "terminal/create", "fs/write_text_file"
    summary                 TEXT NOT NULL,                  -- e.g. "Run command: cargo test"
    details_json            TEXT NOT NULL,                  -- Arguments, diff details and options
    status                  TEXT NOT NULL DEFAULT 'pending',-- 'pending' | 'approved' | 'rejected' | 'cancelled'
    resolved_by             TEXT,                           -- 'cli' | 'web' | 'android_push'
    created_at              INTEGER NOT NULL,
    resolved_at             INTEGER
);

CREATE INDEX idx_permission_pending ON permission_requests(status) WHERE status = 'pending';

-- 8. Elicitation (structured input request from ACP `elicitation/create`).
CREATE TABLE elicitation_requests (
    elicitation_id          TEXT PRIMARY KEY,               -- ACP elicitation id
    session_id              TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    node_id                 TEXT NOT NULL REFERENCES nodes(node_id), -- The node itself on the node side
    message                 TEXT NOT NULL,                  -- Message shown to the user
    mode                    TEXT NOT NULL DEFAULT 'form',   -- 'form' | 'url' (only form is supported)
    schema_json             TEXT NOT NULL DEFAULT '{}',     -- requestedSchema for form mode
    tool_call_id            TEXT,                           -- Related tool call ID (optional)
    status                  TEXT NOT NULL DEFAULT 'pending',-- 'pending' | 'accepted' | 'declined' | 'cancelled'
    content_json            TEXT NOT NULL DEFAULT 'null',   -- Answer content on accept
    resolved_by             TEXT,                           -- 'cli' | 'web' | 'android_push'
    created_at              INTEGER NOT NULL,
    resolved_at             INTEGER
);

CREATE INDEX idx_elicitation_pending ON elicitation_requests(status) WHERE status = 'pending';

-- 9. Web Push (VAPID) subscriptions (hub and standalone node).
CREATE TABLE push_subscriptions (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    endpoint                TEXT NOT NULL UNIQUE,
    p256dh                  TEXT NOT NULL,
    auth                    TEXT NOT NULL,
    device_name             TEXT,                           -- e.g. "Pixel 9 Chrome PWA"
    created_at              INTEGER NOT NULL
);

-- 10. Audit log.
--     Each store records only the operations it observed (not synchronized).
CREATE TABLE audit_logs (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    action                  TEXT NOT NULL,                  -- 'session_start' | 'permission_resolved' | 'pty_spawn' | 'kill_switch' | 'worktree_manage'
    session_id              TEXT,                           -- Related session ID (optional)
    node_id                 TEXT,                           -- Target node ID (optional; the node itself on the node side)
    client_ip               TEXT NOT NULL,                  -- Source IP address (LAN/VPN IP)
    client_user_agent       TEXT,                           -- Client User-Agent
    auth_subject            TEXT NOT NULL,                  -- Token identifier or authentication subject
    details_json            TEXT NOT NULL DEFAULT '{}',     -- Operation details (executed command, approval option, etc.)
    created_at              INTEGER NOT NULL
);

CREATE INDEX idx_audit_logs_action ON audit_logs(action, created_at DESC);
CREATE INDEX idx_audit_logs_session ON audit_logs(session_id, created_at DESC);

-- 11. Node-owned runtime settings (node.db; single-row table).
CREATE TABLE node_settings (
    id                      INTEGER PRIMARY KEY CHECK (id = 1),
    name                    TEXT NOT NULL,
    allow_remote_pty        INTEGER NOT NULL DEFAULT 0,
    project_scan_dirs_json  TEXT NOT NULL DEFAULT '[]',
    tray                    INTEGER,
    vapid_subject           TEXT,                           -- Web Push VAPID contact claim
    revision                INTEGER NOT NULL DEFAULT 1,
    updated_at              INTEGER NOT NULL
);

-- 12. Agent settings, aliases and custom definitions (node.db).
CREATE TABLE agent_settings (
    id                      INTEGER PRIMARY KEY CHECK (id = 1),
    registry_url            TEXT,
    registry_cache_ttl_secs INTEGER,
    revision                INTEGER NOT NULL DEFAULT 1,
    updated_at              INTEGER NOT NULL
);

CREATE TABLE agent_aliases (
    alias                   TEXT PRIMARY KEY,
    agent_id                TEXT NOT NULL,
    updated_at              INTEGER NOT NULL
);

CREATE TABLE custom_agents (
    agent_id                TEXT PRIMARY KEY,
    name                    TEXT,
    command                 TEXT NOT NULL,
    args_json               TEXT NOT NULL DEFAULT '[]',
    env_json                TEXT NOT NULL DEFAULT '{}',
    env_secrets_json        TEXT NOT NULL DEFAULT '{}',
    extension               TEXT,                           -- Optional built-in extension ID
    description             TEXT,
    icon                    TEXT,
    origin_json             TEXT,                           -- Registry import provenance
    capabilities_json       TEXT NOT NULL DEFAULT '{"subagents":true}', -- Advertised ACP client capabilities
    updated_at              INTEGER NOT NULL
);

-- 13. Explicit local path -> logical project link (node.db).
CREATE TABLE project_path_links (
    local_path              TEXT PRIMARY KEY,
    project_key             TEXT NOT NULL,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL
);

CREATE INDEX idx_project_path_links_key
    ON project_path_links(project_key);

-- 14. Hub-owned runtime settings and provisioners (hub.db).
CREATE TABLE hub_settings (
    id                      INTEGER PRIMARY KEY CHECK (id = 1),
    vapid_subject           TEXT,
    revision                INTEGER NOT NULL DEFAULT 1,
    updated_at              INTEGER NOT NULL
);

CREATE TABLE provisioners (
    name                    TEXT PRIMARY KEY,
    description             TEXT,
    command                 TEXT NOT NULL,
    args_json               TEXT NOT NULL DEFAULT '[]',
    idle_timeout_secs       INTEGER,
    env_json                TEXT NOT NULL DEFAULT '{}',
    env_secrets_json        TEXT NOT NULL DEFAULT '{}',     -- Secret IDs read from <data>/hub/secrets/<id> at spawn time
    updated_at              INTEGER NOT NULL
);

-- 15. Secret metadata (node.db / hub.db; values live in secrets/{node,hub}/).
CREATE TABLE secrets (
    id                      TEXT PRIMARY KEY,
    description             TEXT,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL
);

-- 16. Node-owned pairing state (node.db).
--     Only the hub address lives here. The pairing token is a secret and stays
--     in <data>/node/node_token (0600).
CREATE TABLE node_pairing (
    id                      INTEGER PRIMARY KEY CHECK (id = 1),
    hub_url                 TEXT,
    revision                INTEGER NOT NULL DEFAULT 1,
    updated_at              INTEGER NOT NULL
);
