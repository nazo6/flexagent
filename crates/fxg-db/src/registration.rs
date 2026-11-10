//! Upsert for node self-registration and logical project binding.
//!
//! On startup a node upserts its own row into the `nodes` table and upserts project resolution
//! results into `projects` / `project_node_bindings` (the same information source as the
//! `NodeHello` report to the hub). Design: `docs/02-database-schema.md` §3.

use fxg_protocol::common::{NodeLifecycleStatus, SessionStatus};
use fxg_protocol::settings::ProjectIdentity;
use fxg_protocol::util::now_ms;
use sqlx::SqlitePool;

use crate::error::DbError;

/// Registration contents for the `nodes` table (node self-registration / `NodeHello` update).
///
/// `token_hash` / `token_issued_at` are managed by [`set_node_token_hash`]
/// (they are not modified by the regular upsert).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRecord {
    /// Node ID
    pub node_id: String,
    /// Display name
    pub name: String,
    /// OS (`windows` / `linux` / `macos`)
    pub os: String,
    /// Architecture (`x86_64` / `aarch64`)
    pub arch: String,
    /// fxg binary version
    pub version: String,
    /// List of available agent IDs
    pub installed_agents: Vec<String>,
    /// Whether this is an ephemeral VM / container node
    pub is_ephemeral: bool,
    /// Provisioner identifier of the ephemeral node
    pub provisioner: Option<String>,
    /// Lifecycle status
    pub lifecycle_status: NodeLifecycleStatus,
    /// Seconds until idle auto-destruction (for ephemeral nodes)
    pub idle_timeout_secs: Option<u64>,
    /// Whether connected
    pub is_online: bool,
    /// Monotonically increasing revision of the node's reported state
    pub state_revision: u64,
}

impl NodeRecord {
    /// Creates a resident, ready, online registration with only the required fields.
    pub fn new(
        node_id: impl Into<String>,
        name: impl Into<String>,
        os: impl Into<String>,
        arch: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            name: name.into(),
            os: os.into(),
            arch: arch.into(),
            version: version.into(),
            installed_agents: Vec::new(),
            is_ephemeral: false,
            provisioner: None,
            lifecycle_status: NodeLifecycleStatus::Ready,
            idle_timeout_secs: None,
            is_online: true,
            state_revision: 0,
        }
    }

    /// Sets the list of available agents.
    pub fn with_agents(mut self, agents: Vec<String>) -> Self {
        self.installed_agents = agents;
        self
    }
}

/// Registration contents for the `projects` table (logical project).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    /// Logical project ID (normalization key)
    pub project_id: String,
    /// Display name
    pub name: String,
    /// Git URL before normalization
    pub canonical_git_url: Option<String>,
    /// How the identity was derived
    pub identity: ProjectIdentity,
}

/// Registration contents for `project_node_bindings` (project × node local path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBindingRecord {
    /// Logical project ID
    pub project_id: String,
    /// Node ID
    pub node_id: String,
    /// Local path on the node
    pub local_path: String,
    /// Whether this is a Git Worktree
    pub is_worktree: bool,
    /// Whether it actually exists on the node at registration time (directory check)
    pub path_exists: bool,
    /// Git branch at the last check
    pub git_branch: Option<String>,
}

/// Upserts node information (`last_seen_at` is updated to the current time).
///
/// `token_hash` / `token_issued_at` / `created_at` keep their existing values.
/// `provisioner` / `idle_timeout_secs` are not overwritten with `NULL` from existing values
/// (so that they are not lost when a NodeHello upsert with `provisioner = None` arrives after
/// the hub pre-registered an ephemeral node).
pub async fn upsert_node(pool: &SqlitePool, record: &NodeRecord) -> Result<(), DbError> {
    let now = now_ms();
    let installed_agents_json = serde_json::to_string(&record.installed_agents)?;
    let idle_timeout_secs = record
        .idle_timeout_secs
        .map(|secs| i64::try_from(secs).unwrap_or(i64::MAX));
    sqlx::query!(
        r#"
        INSERT INTO nodes (
            node_id, name, os, arch, version, installed_agents_json, is_ephemeral,
            provisioner, lifecycle_status, idle_timeout_secs, is_online, last_seen_at,
            created_at, state_revision
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(node_id) DO UPDATE SET
            name = excluded.name,
            os = excluded.os,
            arch = excluded.arch,
            version = excluded.version,
            installed_agents_json = excluded.installed_agents_json,
            is_ephemeral = excluded.is_ephemeral,
            -- Preserve the provisioner / idle_timeout that the hub pre-registered for an
            -- ephemeral node, so the NodeHello upsert (provisioner = NULL) cannot lose them
            provisioner = COALESCE(excluded.provisioner, nodes.provisioner),
            lifecycle_status = excluded.lifecycle_status,
            idle_timeout_secs = COALESCE(excluded.idle_timeout_secs, nodes.idle_timeout_secs),
            is_online = excluded.is_online,
            last_seen_at = excluded.last_seen_at,
            state_revision = MAX(nodes.state_revision, excluded.state_revision)
        "#,
        record.node_id,
        record.name,
        record.os,
        record.arch,
        record.version,
        installed_agents_json,
        record.is_ephemeral,
        record.provisioner.as_deref(),
        record.lifecycle_status.as_str(),
        idle_timeout_secs,
        record.is_online,
        now,
        now,
        i64::try_from(record.state_revision).unwrap_or(i64::MAX),
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Updates the node's lifecycle state (ephemeral VM provisioning / terminated, etc.).
pub async fn set_node_lifecycle(
    pool: &SqlitePool,
    node_id: &str,
    status: NodeLifecycleStatus,
) -> Result<(), DbError> {
    let result = sqlx::query!(
        r#"UPDATE nodes SET lifecycle_status = ? WHERE node_id = ?"#,
        status.as_str(),
        node_id,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NodeNotFound(node_id.to_owned()));
    }
    Ok(())
}

/// Updates the session state while an ephemeral VM is being provisioned (hub only).
///
/// This updates a provisional projection row rather than an event, and is used only to record
/// bootstrap failures and timeouts (event-derived state is authoritative via the executing
/// node's `StatusChanged`).
pub async fn set_provisional_session_status(
    pool: &SqlitePool,
    session_id: &str,
    status: SessionStatus,
) -> Result<(), DbError> {
    let result = sqlx::query!(
        r#"UPDATE sessions SET status = ?, updated_at = ? WHERE session_id = ?"#,
        status.as_str(),
        now_ms(),
        session_id,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::SessionNotFound(session_id.to_owned()));
    }
    Ok(())
}

/// Updates the node's online state (`last_seen_at` is also updated).
pub async fn set_node_online(
    pool: &SqlitePool,
    node_id: &str,
    is_online: bool,
) -> Result<(), DbError> {
    let result = sqlx::query!(
        r#"UPDATE nodes SET is_online = ?, last_seen_at = ? WHERE node_id = ?"#,
        is_online,
        now_ms(),
        node_id,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NodeNotFound(node_id.to_owned()));
    }
    Ok(())
}

/// Sets the hash of a per-node token (`None` revokes it).
///
/// Plaintext tokens are never stored (design: `docs/01-architecture-and-sync.md` §7.3).
pub async fn set_node_token_hash(
    pool: &SqlitePool,
    node_id: &str,
    token_hash: Option<&str>,
) -> Result<(), DbError> {
    let issued_at = token_hash.map(|_| now_ms());
    let result = sqlx::query!(
        r#"UPDATE nodes SET token_hash = ?, token_issued_at = ? WHERE node_id = ?"#,
        token_hash,
        issued_at,
        node_id,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NodeNotFound(node_id.to_owned()));
    }
    Ok(())
}

/// Returns the list of per-node token hashes `(node_id, token_hash)` (for hub authentication).
///
/// Because plaintext tokens are never stored, the hub hashes the presented token and compares
/// it in constant time (design: `docs/01-architecture-and-sync.md` §7.3).
pub async fn list_node_tokens(pool: &SqlitePool) -> Result<Vec<(String, String)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT node_id, token_hash
          FROM nodes
         WHERE token_hash IS NOT NULL
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut tokens = Vec::with_capacity(rows.len());
    for row in rows {
        // SQLite prepared statements cannot report NOT NULL, so sqlx generates Option
        // (the WHERE clause's IS NOT NULL is guaranteed at runtime)
        if let (Some(node_id), Some(hash)) = (row.node_id, row.token_hash) {
            tokens.push((node_id, hash));
        }
    }
    Ok(tokens)
}

/// Deletes a project × node local path binding (e.g. when a Worktree is deleted).
///
/// Returns the number of deleted rows.
pub async fn delete_project_binding(
    pool: &SqlitePool,
    node_id: &str,
    local_path: &str,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM project_node_bindings
         WHERE node_id = ? AND local_path = ?
        "#,
        node_id,
        local_path,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Deletes bindings of other projects that share the same `(node_id, local_path)`.
///
/// Used to avoid leaving old bindings when relinking a project (Web UI).
/// Returns the number of deleted rows.
pub async fn delete_other_project_bindings(
    pool: &SqlitePool,
    node_id: &str,
    local_path: &str,
    keep_project_id: &str,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM project_node_bindings
         WHERE node_id = ? AND local_path = ? AND project_id != ?
        "#,
        node_id,
        local_path,
        keep_project_id,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Deletes orphan projects that have neither bindings nor sessions (returns the number of
/// deleted rows).
///
/// On the hub side this guards against deleting projects referenced by sessions.
pub async fn delete_orphan_projects(pool: &SqlitePool) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM projects
         WHERE NOT EXISTS (
                   SELECT 1 FROM project_node_bindings b
                    WHERE b.project_id = projects.project_id
               )
           AND NOT EXISTS (
                   SELECT 1 FROM sessions s
                    WHERE s.project_id = projects.project_id
               )
        "#,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Upserts a logical project.
///
/// The existing row's `canonical_git_url` is not overwritten with `None`
/// (handles the case where the Git URL is discovered later through fallback resolution).
pub async fn upsert_project(pool: &SqlitePool, record: &ProjectRecord) -> Result<(), DbError> {
    let now = now_ms();
    sqlx::query!(
        r#"
        INSERT INTO projects
            (project_id, name, canonical_git_url, identity_kind, project_key, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(project_id) DO UPDATE SET
            name = excluded.name,
            canonical_git_url = COALESCE(excluded.canonical_git_url, projects.canonical_git_url),
            identity_kind = excluded.identity_kind,
            project_key = COALESCE(excluded.project_key, projects.project_key),
            updated_at = excluded.updated_at
        "#,
        record.project_id,
        record.name,
        record.canonical_git_url.as_deref(),
        record.identity.kind.as_str(),
        record.identity.key.as_deref(),
        now,
        now,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Upserts a project × node local path binding.
///
/// If the `projects` / `nodes` rows do not exist, this violates the FK constraint, so callers
/// must run [`upsert_project`] / [`upsert_node`] first.
pub async fn upsert_project_binding(
    pool: &SqlitePool,
    record: &ProjectBindingRecord,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO project_node_bindings
            (project_id, node_id, local_path, is_worktree, path_exists, git_branch, last_used_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(project_id, node_id, local_path) DO UPDATE SET
            is_worktree = excluded.is_worktree,
            path_exists = excluded.path_exists,
            git_branch = excluded.git_branch,
            last_used_at = excluded.last_used_at
        "#,
        record.project_id,
        record.node_id,
        record.local_path,
        record.is_worktree,
        record.path_exists,
        record.git_branch.as_deref(),
        now_ms(),
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Provisional session row while an ephemeral VM is being provisioned.
///
/// An ephemeral VM goes through `bootstrap-workspace` (git clone + tool installation) before
/// `fxg internal stdio` sends `SessionCreated`, so until then the client (UI) needs to see the
/// session's existence and its `provisioning` state. The hub does this by directly
/// inserting a projection row rather than an event (preserving the single write authority), and
/// the subsequent `SessionCreated` projection upsert overwrites the derived columns (`status` is
/// retained until `StatusChanged`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionalSessionRecord {
    /// Session ID (assigned by the hub)
    pub session_id: String,
    /// Logical project ID
    pub project_id: String,
    /// Project display name (used to create the `projects` row if absent)
    pub project_name: String,
    /// Executing node ID (the pre-assigned ID of the ephemeral VM node)
    pub node_id: String,
    /// Execution directory (local path from the start request; for UI display)
    pub local_path: String,
    /// Git branch (optional)
    pub git_branch: Option<String>,
    /// Agent ID to start
    pub agent_id: String,
    /// Display title
    pub title: String,
}

/// Inserts a provisional session row while an ephemeral VM is provisioning (idempotent).
///
/// Does nothing if the `sessions` row already exists (the node has already sent
/// `SessionCreated`). Callers must register the ephemeral node ID with [`upsert_node`]
/// beforehand.
pub async fn upsert_provisional_session(
    pool: &SqlitePool,
    record: &ProvisionalSessionRecord,
) -> Result<(), DbError> {
    let now = now_ms();
    // Prepare an unknown project to satisfy the FK on `sessions.project_id`
    sqlx::query!(
        r#"
        INSERT INTO projects (project_id, name, canonical_git_url, created_at, updated_at)
        VALUES (?, ?, NULL, ?, ?)
        ON CONFLICT(project_id) DO NOTHING
        "#,
        record.project_id,
        record.project_name,
        now,
        now,
    )
    .execute(pool)
    .await?;

    sqlx::query!(
        r#"
        INSERT INTO sessions (
            session_id, project_id, node_id, local_path, git_branch, is_worktree,
            agent_id, agent_session_id, parent_session_id,
            title, status, current_mode, available_modes_json,
            available_commands_json, config_options_json,
            last_node_seq, synced_up_to_node_seq, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, 0, ?, NULL, NULL, ?, ?, NULL, '[]', '[]', '[]', 0, 0, ?, ?)
        ON CONFLICT(session_id) DO NOTHING
        "#,
        record.session_id,
        record.project_id,
        record.node_id,
        record.local_path,
        record.git_branch.as_deref(),
        record.agent_id,
        record.title,
        SessionStatus::Provisioning.as_str(),
        now,
        now,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Returns the `state_revision` recorded for a node.
pub async fn node_state_revision(pool: &SqlitePool, node_id: &str) -> Result<Option<u64>, DbError> {
    let row = sqlx::query!(
        r#"SELECT state_revision FROM nodes WHERE node_id = ?"#,
        node_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| u64::try_from(row.state_revision).unwrap_or(0)))
}

/// Deletes the project bindings of a node that are absent from a complete report.
///
/// `keep_local_paths` are the bindings contained in the latest `NodeHello`; every
/// other binding of the node is removed (Worktree removal, `project_key` changes).
pub async fn prune_node_project_bindings(
    pool: &SqlitePool,
    node_id: &str,
    keep_local_paths: &[String],
) -> Result<u64, DbError> {
    let rows = sqlx::query!(
        r#"SELECT local_path FROM project_node_bindings WHERE node_id = ?"#,
        node_id,
    )
    .fetch_all(pool)
    .await?;
    let mut deleted = 0u64;
    for row in rows {
        if keep_local_paths.iter().any(|path| path == &row.local_path) {
            continue;
        }
        deleted += delete_project_binding(pool, node_id, &row.local_path).await?;
    }
    Ok(deleted)
}

/// Reassigns the sessions of a node by local path (binding changes, `project_key`).
pub async fn reassign_sessions_project(
    pool: &SqlitePool,
    node_id: &str,
    assignments: &[(String, String)],
) -> Result<u64, DbError> {
    let mut updated = 0u64;
    for (local_path, project_id) in assignments {
        let result = sqlx::query!(
            r#"
            UPDATE sessions
               SET project_id = ?, updated_at = ?
             WHERE node_id = ? AND local_path = ? AND project_id != ?
            "#,
            project_id,
            now_ms(),
            node_id,
            local_path,
            project_id,
        )
        .execute(pool)
        .await?;
        updated += result.rows_affected();
    }
    Ok(updated)
}

/// Reassigns a single session to a logical project (used by `SessionSyncState`).
pub async fn reassign_session_project(
    pool: &SqlitePool,
    session_id: &str,
    project_id: &str,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE sessions
           SET project_id = ?, updated_at = ?
         WHERE session_id = ? AND project_id != ?
        "#,
        project_id,
        now_ms(),
        session_id,
        project_id,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Increments `nodes.state_revision` and returns the new value.
pub async fn bump_node_state_revision(pool: &SqlitePool, node_id: &str) -> Result<u64, DbError> {
    let row = sqlx::query!(
        r#"
        UPDATE nodes
           SET state_revision = state_revision + 1
         WHERE node_id = ?
        RETURNING state_revision
        "#,
        node_id,
    )
    .fetch_optional(pool)
    .await?;
    row.map(|row| u64::try_from(row.state_revision).unwrap_or(0))
        .ok_or_else(|| DbError::NodeNotFound(node_id.to_owned()))
}
