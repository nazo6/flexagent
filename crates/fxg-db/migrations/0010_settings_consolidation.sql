-- Settings consolidation: DB-owned settings, project identity and secret metadata.
--
-- Everything changed at runtime is owned by exactly one DB:
--   node.db   : node_settings / agent_settings / agent_aliases / custom_agents /
--               enabled_package_agents / project_path_links
--   server.db : server_settings / git_credentials / provisioners / project_profiles
-- Secret *values* never live in the DB: only metadata is stored here, while the
-- value file lives under ~/.flexagent/secrets/{node,server}/<id>.

-- 1. Node settings (node.db; single-row table)
CREATE TABLE node_settings (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    name                     TEXT NOT NULL,
    allow_remote_pty         INTEGER NOT NULL DEFAULT 0,
    project_scan_dirs_json   TEXT NOT NULL DEFAULT '[]',
    tray                     INTEGER,
    revision                 INTEGER NOT NULL DEFAULT 1,
    updated_at               INTEGER NOT NULL
);

-- 2. Agent settings, aliases and custom definitions (node.db)
CREATE TABLE agent_settings (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    default_agent            TEXT,
    registry_url             TEXT,
    registry_cache_ttl_secs  INTEGER,
    revision                 INTEGER NOT NULL DEFAULT 1,
    updated_at               INTEGER NOT NULL
);

CREATE TABLE agent_aliases (
    alias                    TEXT PRIMARY KEY,
    agent_id                 TEXT NOT NULL,
    updated_at               INTEGER NOT NULL
);

CREATE TABLE custom_agents (
    agent_id                 TEXT PRIMARY KEY,
    name                     TEXT,
    command                  TEXT NOT NULL,
    args_json                TEXT NOT NULL DEFAULT '[]',
    env_json                 TEXT NOT NULL DEFAULT '{}',
    env_secrets_json         TEXT NOT NULL DEFAULT '{}',
    updated_at               INTEGER NOT NULL
);

CREATE TABLE enabled_package_agents (
    agent_id                 TEXT PRIMARY KEY,
    updated_at               INTEGER NOT NULL
);

-- 3. Explicit local path -> logical project link (node.db)
CREATE TABLE project_path_links (
    local_path               TEXT PRIMARY KEY,
    project_key              TEXT NOT NULL,
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL
);

CREATE INDEX idx_project_path_links_key
    ON project_path_links(project_key);

-- 4. Server settings, Git credentials, provisioners and VM profiles (server.db)
CREATE TABLE server_settings (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    vapid_subject            TEXT,
    revision                 INTEGER NOT NULL DEFAULT 1,
    updated_at               INTEGER NOT NULL
);

CREATE TABLE git_credentials (
    host                     TEXT PRIMARY KEY,
    provider                 TEXT NOT NULL,
    username                 TEXT,
    token_env                TEXT,
    secret_id                TEXT,
    updated_at               INTEGER NOT NULL
);

CREATE TABLE provisioners (
    name                     TEXT PRIMARY KEY,
    description              TEXT,
    command                  TEXT NOT NULL,
    args_json                TEXT NOT NULL DEFAULT '[]',
    idle_timeout_secs        INTEGER,
    env_json                 TEXT NOT NULL DEFAULT '{}',
    updated_at               INTEGER NOT NULL
);

CREATE TABLE project_profiles (
    project_id               TEXT PRIMARY KEY REFERENCES projects(project_id) ON DELETE CASCADE,
    bootstrap_setup_json     TEXT NOT NULL DEFAULT '[]',
    bootstrap_env_json       TEXT NOT NULL DEFAULT '{}',
    revision                 INTEGER NOT NULL DEFAULT 1,
    updated_at               INTEGER NOT NULL
);

-- 5. Secret metadata (node.db / server.db; values live in secrets/{node,server}/)
CREATE TABLE secrets (
    id                       TEXT PRIMARY KEY,
    name                     TEXT NOT NULL,
    kind                     TEXT NOT NULL,
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL
);

-- 6. Existing tables
ALTER TABLE projects ADD COLUMN identity_kind TEXT NOT NULL DEFAULT 'local';
ALTER TABLE projects ADD COLUMN project_key TEXT;
ALTER TABLE nodes ADD COLUMN state_revision INTEGER NOT NULL DEFAULT 0;
