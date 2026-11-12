//! Shared client-facing query layer.
//!
//! Lets both `node.db` (local direct UI / CLI) and `hub.db` (hub UI) return
//! **the same response types** ([`SessionSummary`] / [`PermissionRequestEntry`], etc.)
//! (design: `docs/02-database-schema.md` §3).
//!
//! # Policy: avoiding dynamic SQL
//!
//! To keep the type-safe macros (`query!` / `query_as!` / `query_scalar!`), list filters are
//! applied in Rust after fetching instead of dynamically composing SQL
//! (a simplification assuming self-hosted, single-tenant scale).
//!
//! The only exceptions are FTS5 `MATCH` and the LIKE fallback for search terms shorter than
//! 3 characters. Both are special syntaxes that cannot go through placeholders, so
//! `query_as` (runtime strings) is used with an explanatory comment (a documented exception,
//! covered by unit tests).
//!
//! # Type overrides
//!
//! SQLite does not add an implicit NOT NULL to `TEXT PRIMARY KEY` columns, so primary key
//! columns are marked non-NULL with `AS "col!"`, and `INTEGER` boolean columns are marked as
//! bool with `AS "col: bool"`.

#![allow(
    clippy::map_err_ignore,
    reason = "integer conversion errors (TryFromIntError) carry no additional context"
)]

use fxg_protocol::common::{
    AuditAction, AuditLogEntry, ElicitationMode, ElicitationRequestEntry, ElicitationRequestStatus,
    EventType, ForkScope, NodeLifecycleStatus, NodeSummary, PermissionPolicy,
    PermissionRequestEntry, PermissionRequestStatus, ProjectBindingSummary, ProjectSummary,
    ResolvedBy, SearchHit, SessionStatus, SessionSummary, SessionUsage,
};
use fxg_protocol::events::SessionEventBatch;
use fxg_protocol::settings::ProjectIdentityKind;
use fxg_protocol::{EventId, NodeId, ProjectId, SessionId};
use sqlx::SqlitePool;

use crate::error::DbError;
use crate::events::{StoredPermissionDetails, envelope_from_row};

/// Filter conditions for the session list.
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Filter by logical project ID
    pub project_id: Option<ProjectId>,
    /// Filter by node ID
    pub node_id: Option<NodeId>,
    /// Filter by session status (all statuses if empty)
    pub statuses: Vec<SessionStatus>,
    /// Whether to include archived sessions (default `false` = excluded)
    pub include_archived: bool,
    /// Maximum number of results
    pub limit: Option<u32>,
}

/// Context character count for LIKE search and snippet generation.
const SNIPPET_CONTEXT_CHARS: usize = 40;

/// Retrieves the session list (descending by update time).
///
/// Deleted (tombstone) sessions are always excluded. Archived sessions are excluded when
/// [`SessionFilter::include_archived`] is `false`.
pub async fn list_sessions(
    pool: &SqlitePool,
    filter: &SessionFilter,
) -> Result<Vec<SessionSummary>, DbError> {
    let rows = sqlx::query_as!(
        SessionRow,
        r#"
        SELECT session_id AS "session_id!",
               project_id, node_id, local_path, git_branch,
               is_worktree AS "is_worktree: bool",
               agent_id, agent_session_id, parent_session_id AS "parent_session_id: SessionId",
               title, status AS "status: SessionStatus", current_mode,
               permission_policy AS "permission_policy: PermissionPolicy", usage_json,
               last_node_seq, fork_scope AS "fork_scope: ForkScope", created_at, updated_at,
               archived_at
          FROM sessions
         WHERE deleted_at IS NULL
         ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut sessions = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(project_id) = filter.project_id.as_ref()
            && &row.project_id != project_id
        {
            continue;
        }
        if let Some(node_id) = filter.node_id.as_ref()
            && &row.node_id != node_id
        {
            continue;
        }
        let summary = row.into_summary()?;
        if !filter.include_archived && summary.archived_at.is_some() {
            continue;
        }
        if !filter.statuses.is_empty() && !filter.statuses.contains(&summary.status) {
            continue;
        }
        sessions.push(summary);
    }
    sessions.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.session_id.as_str().cmp(b.session_id.as_str()))
    });
    if let Some(limit) = filter.limit {
        sessions.truncate(limit as usize);
    }
    Ok(sessions)
}

/// Retrieves a single session (`None` if deleted).
pub async fn get_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<SessionSummary>, DbError> {
    let row = sqlx::query_as!(
        SessionRow,
        r#"
        SELECT session_id AS "session_id!",
               project_id, node_id, local_path, git_branch,
               is_worktree AS "is_worktree: bool",
               agent_id, agent_session_id, parent_session_id AS "parent_session_id: SessionId",
               title, status AS "status: SessionStatus", current_mode,
               permission_policy AS "permission_policy: PermissionPolicy", usage_json,
               last_node_seq, fork_scope AS "fork_scope: ForkScope", created_at, updated_at,
               archived_at
          FROM sessions
         WHERE session_id = ? AND deleted_at IS NULL
        "#,
        session_id,
    )
    .fetch_optional(pool)
    .await?;
    row.map(SessionRow::into_summary).transpose()
}

/// Retrieves the event history after `after_cursor` in batches (for resuming incremental sync).
///
/// The returned batch's `cursor` is the cursor of the last event that could be retrieved; if
/// there are no matching events, `after_cursor` is returned unchanged.
pub async fn session_events_after(
    pool: &SqlitePool,
    session_id: &str,
    after_cursor: u64,
    limit: u32,
) -> Result<SessionEventBatch, DbError> {
    let after = i64::try_from(after_cursor).map_err(|_| DbError::CursorOutOfRange(after_cursor))?;
    let rows = sqlx::query!(
        r#"
        SELECT cursor, event_id, session_id, node_seq, payload_json, created_at
          FROM session_events
         WHERE session_id = ? AND cursor > ?
         ORDER BY cursor ASC
         LIMIT ?
        "#,
        session_id,
        after,
        i64::from(limit),
    )
    .fetch_all(pool)
    .await?;

    let mut cursor = after_cursor;
    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        cursor = u64::try_from(row.cursor).map_err(|_| DbError::NegativeNodeSeq(row.cursor))?;
        events.push(envelope_from_row(
            row.event_id,
            row.session_id,
            row.node_seq,
            row.payload_json,
            row.created_at,
        )?);
    }
    Ok(SessionEventBatch { events, cursor })
}

/// Retrieves the most recent events before `before_cursor` (oldest first).
///
/// `None` means "the end of the log" and is used for the tail shown by
/// `fxg session show` / `GET /api/v1/sessions/:id`.
pub async fn session_events_before(
    pool: &SqlitePool,
    session_id: &str,
    before_cursor: Option<u64>,
    limit: u32,
) -> Result<SessionEventBatch, DbError> {
    let before = match before_cursor {
        Some(cursor) => i64::try_from(cursor).map_err(|_| DbError::CursorOutOfRange(cursor))?,
        None => i64::MAX,
    };
    let rows = sqlx::query!(
        r#"
        SELECT cursor, event_id, session_id, node_seq, payload_json, created_at
          FROM session_events
         WHERE session_id = ? AND cursor < ?
         ORDER BY cursor DESC
         LIMIT ?
        "#,
        session_id,
        before,
        i64::from(limit),
    )
    .fetch_all(pool)
    .await?;

    let rows = rows
        .into_iter()
        .rev()
        .map(|row| {
            (
                row.cursor,
                row.event_id,
                row.session_id,
                row.node_seq,
                row.payload_json,
                row.created_at,
            )
        })
        .collect();
    batch_from_rows(rows, 0)
}

/// Builds a [`SessionEventBatch`] from `(cursor, event_id, session_id, node_seq, payload_json,
/// created_at)` rows (shared routine).
fn batch_from_rows(
    rows: Vec<(i64, String, String, i64, String, i64)>,
    after_cursor: u64,
) -> Result<SessionEventBatch, DbError> {
    let mut cursor = after_cursor;
    let mut events = Vec::with_capacity(rows.len());
    for (row_cursor, event_id, session_id, node_seq, payload_json, created_at) in rows {
        cursor = u64::try_from(row_cursor).map_err(|_| DbError::NegativeNodeSeq(row_cursor))?;
        events.push(envelope_from_row(
            event_id,
            session_id,
            node_seq,
            payload_json,
            created_at,
        )?);
    }
    Ok(SessionEventBatch { events, cursor })
}

/// Retrieves events after `after_cursor` across all sessions.
///
/// Used for Client WS replay (`Subscribe { since_cursor }`) and outbox resumption.
/// The returned batch's `cursor` is the cursor of the last event retrieved.
pub async fn events_after_cursor(
    pool: &SqlitePool,
    after_cursor: u64,
    limit: u32,
) -> Result<SessionEventBatch, DbError> {
    let after = i64::try_from(after_cursor).map_err(|_| DbError::CursorOutOfRange(after_cursor))?;
    let rows = sqlx::query!(
        r#"
        SELECT cursor, event_id, session_id, node_seq, payload_json, created_at
          FROM session_events
         WHERE cursor > ?
         ORDER BY cursor ASC
         LIMIT ?
        "#,
        after,
        i64::from(limit),
    )
    .fetch_all(pool)
    .await?;

    let rows = rows
        .into_iter()
        .map(|row| {
            (
                row.cursor,
                row.event_id,
                row.session_id,
                row.node_seq,
                row.payload_json,
                row.created_at,
            )
        })
        .collect();
    batch_from_rows(rows, after_cursor)
}

/// Returns the latest cursor of `session_events` (0 when there are no events).
///
/// Used to detect when Client WS `Subscribe { since_cursor }` exceeds the end of the store
/// (stale cursor left after DB recreation / reset).
pub async fn latest_cursor(pool: &SqlitePool) -> Result<u64, DbError> {
    let cursor = sqlx::query_scalar!(
        r#"
        SELECT MAX(cursor)
          FROM session_events
        "#,
    )
    .fetch_one(pool)
    .await?;
    match cursor {
        Some(value) => u64::try_from(value).map_err(|_| DbError::NegativeNodeSeq(value)),
        None => Ok(0),
    }
}

/// Retrieves the logical project list (including node / Worktree bindings).
pub async fn list_projects(pool: &SqlitePool) -> Result<Vec<ProjectSummary>, DbError> {
    let project_rows = sqlx::query_as!(
        ProjectRow,
        r#"
        SELECT project_id AS "project_id!",
               name, canonical_git_url,
               identity_kind AS "identity_kind!: ProjectIdentityKind",
               project_key, created_at, updated_at
          FROM projects
         ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let binding_rows = sqlx::query_as!(
        BindingRow,
        r#"
        SELECT project_id, node_id, local_path,
               is_worktree AS "is_worktree: bool",
               path_exists AS "path_exists: bool",
               git_branch, last_used_at
          FROM project_node_bindings
         ORDER BY last_used_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut projects: Vec<ProjectSummary> = project_rows
        .into_iter()
        .map(|row| ProjectSummary {
            project_id: row.project_id,
            name: row.name,
            canonical_git_url: row.canonical_git_url,
            identity: fxg_protocol::settings::ProjectIdentity {
                kind: row.identity_kind,
                key: row.project_key,
            },
            created_at: row.created_at,
            updated_at: row.updated_at,
            bindings: Vec::new(),
        })
        .collect();

    for binding in binding_rows {
        if let Some(project) = projects
            .iter_mut()
            .find(|project| project.project_id == binding.project_id)
        {
            project.bindings.push(ProjectBindingSummary {
                node_id: binding.node_id,
                local_path: binding.local_path,
                is_worktree: binding.is_worktree,
                path_exists: binding.path_exists,
                git_branch: binding.git_branch,
                last_used_at: binding.last_used_at,
            });
        }
    }
    Ok(projects)
}

/// Resolves the default execution directory path.
///
/// # Resolution order
///
/// 1. Among the `(project_id, node_id)` bindings, the one **most recently used**
///    (including Worktrees; see [`select_default_binding_path`])
/// 2. If there is no binding, the `local_path` of the latest `(project_id, node_id)` session
///    (fallback for sessions executed before binding registration on session start was
///    implemented)
///
/// Used as the default when `local_path` is omitted in a session start request, shared by both
/// the node and the hub (design: docs/01-architecture-and-sync.md §4).
pub async fn resolve_default_local_path(
    pool: &SqlitePool,
    project_id: &ProjectId,
    node_id: &NodeId,
) -> Result<Option<String>, DbError> {
    let projects = list_projects(pool).await?;
    if let Some(project) = projects
        .iter()
        .find(|project| &project.project_id == project_id)
        && let Some(path) = select_default_binding_path(&project.bindings, node_id)
    {
        return Ok(Some(path));
    }

    let sessions = list_sessions(
        pool,
        &SessionFilter {
            project_id: Some(project_id.clone()),
            node_id: Some(node_id.clone()),
            limit: Some(1),
            ..SessionFilter::default()
        },
    )
    .await?;
    Ok(sessions
        .into_iter()
        .next()
        .map(|session| session.local_path))
}

/// Chooses the default path from a list of bindings.
///
/// Chooses the most recently used (`last_used_at`) among **existing** bindings. Worktrees are
/// also candidates (the most recent working location becomes the default). On ties, the main
/// repository (`is_worktree = false`) is preferred.
///
/// If there is no existing binding, returns the most recently used binding
/// (the UI is responsible for UX warnings so that non-existent paths are not used silently).
pub fn select_default_binding_path(
    bindings: &[ProjectBindingSummary],
    node_id: &NodeId,
) -> Option<String> {
    let candidates: Vec<&ProjectBindingSummary> = bindings
        .iter()
        .filter(|binding| &binding.node_id == node_id)
        .collect();
    let latest_existing = candidates
        .iter()
        .filter(|binding| binding.path_exists)
        .max_by_key(|binding| (binding.last_used_at, !binding.is_worktree));
    latest_existing
        .copied()
        .or_else(|| {
            candidates
                .into_iter()
                .max_by_key(|binding| (binding.last_used_at, !binding.is_worktree))
        })
        .map(|binding| binding.local_path.clone())
}

/// Retrieves the node list (including online state and ephemeral node attributes).
pub async fn list_nodes(pool: &SqlitePool) -> Result<Vec<NodeSummary>, DbError> {
    let rows = sqlx::query_as!(
        NodeRow,
        r#"
        SELECT node_id AS "node_id!",
               name, os, arch, version, installed_agents_json,
               is_ephemeral AS "is_ephemeral: bool",
               provisioner, lifecycle_status AS "lifecycle_status: NodeLifecycleStatus",
               is_online AS "is_online: bool",
               last_seen_at
          FROM nodes
         ORDER BY last_seen_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let installed_agents: Vec<String> = serde_json::from_str(&row.installed_agents_json)
                .map_err(|err| {
                    DbError::InvalidPayload(format!(
                        "nodes.installed_agents_json for {}: {err}",
                        row.node_id
                    ))
                })?;
            Ok(NodeSummary {
                node_id: row.node_id,
                name: row.name,
                os: row.os,
                arch: row.arch,
                version: row.version,
                is_ephemeral: row.is_ephemeral,
                provisioner: row.provisioner,
                lifecycle_status: row.lifecycle_status,
                is_online: row.is_online,
                last_seen_at: row.last_seen_at,
                installed_agents,
            })
        })
        .collect()
}

/// Retrieves unresolved (`pending`) permission requests (global approval inbox).
pub async fn pending_permissions(
    pool: &SqlitePool,
) -> Result<Vec<PermissionRequestEntry>, DbError> {
    let rows = sqlx::query_as!(
        PermissionRow,
        r#"
        SELECT request_id AS "request_id!",
               session_id, node_id, tool_name, summary, details_json,
               status AS "status: PermissionRequestStatus", created_at, resolved_at,
               resolved_by AS "resolved_by: ResolvedBy"
          FROM permission_requests
         WHERE status = ?
         ORDER BY created_at DESC
        "#,
        PermissionRequestStatus::Pending.as_str(),
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(PermissionRow::into_entry).collect()
}

/// Retrieves a single permission request (used e.g. to determine the response to
/// `PermissionResolved`).
pub async fn find_permission_request(
    pool: &SqlitePool,
    request_id: &str,
) -> Result<Option<PermissionRequestEntry>, DbError> {
    let row = sqlx::query_as!(
        PermissionRow,
        r#"
        SELECT request_id AS "request_id!",
               session_id, node_id, tool_name, summary, details_json,
               status AS "status: PermissionRequestStatus", created_at, resolved_at,
               resolved_by AS "resolved_by: ResolvedBy"
          FROM permission_requests
         WHERE request_id = ?
        "#,
        request_id,
    )
    .fetch_optional(pool)
    .await?;
    row.map(PermissionRow::into_entry).transpose()
}

/// Retrieves unresolved (`pending`) elicitation requests (question inbox).
pub async fn pending_elicitations(
    pool: &SqlitePool,
) -> Result<Vec<ElicitationRequestEntry>, DbError> {
    let rows = sqlx::query_as!(
        ElicitationRow,
        r#"
        SELECT elicitation_id AS "elicitation_id!",
               session_id, node_id, message, mode AS "mode: ElicitationMode", schema_json,
               tool_call_id, status AS "status: ElicitationRequestStatus", content_json,
               created_at, resolved_at, resolved_by AS "resolved_by: ResolvedBy"
          FROM elicitation_requests
         WHERE status = ?
         ORDER BY created_at DESC
        "#,
        ElicitationRequestStatus::Pending.as_str(),
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(ElicitationRow::into_entry).collect()
}

/// Retrieves a single elicitation request (used e.g. to determine the response to
/// `ElicitationResolved`).
pub async fn find_elicitation_request(
    pool: &SqlitePool,
    elicitation_id: &str,
) -> Result<Option<ElicitationRequestEntry>, DbError> {
    let row = sqlx::query_as!(
        ElicitationRow,
        r#"
        SELECT elicitation_id AS "elicitation_id!",
               session_id, node_id, message, mode AS "mode: ElicitationMode", schema_json,
               tool_call_id, status AS "status: ElicitationRequestStatus", content_json,
               created_at, resolved_at, resolved_by AS "resolved_by: ResolvedBy"
          FROM elicitation_requests
         WHERE elicitation_id = ?
        "#,
        elicitation_id,
    )
    .fetch_optional(pool)
    .await?;
    row.map(ElicitationRow::into_entry).transpose()
}

/// FTS5 full-text search. Search terms shorter than 3 characters fall back to LIKE.
///
/// The trigram tokenizer does not match search terms shorter than 3 characters
/// (design: `docs/02-database-schema.md` §3).
pub async fn search(pool: &SqlitePool, query: &str, limit: u32) -> Result<Vec<SearchHit>, DbError> {
    let needle = query.trim();
    if needle.is_empty() {
        return Err(DbError::EmptySearchQuery);
    }
    let limit = i64::from(limit.clamp(1, 500));

    if needle.chars().count() < 3 {
        return search_like(pool, needle, limit).await;
    }

    // FTS5 MATCH requires embedding phrase quotes into the SQL string, which cannot be
    // expressed with placeholders alone (a documented exception).
    // The search term is passed as a double-quoted phrase to avoid errors from FTS5 operator
    // interpretation of `-`, `"`, etc.
    let match_query = format!("\"{}\"", needle.replace('"', "\"\""));
    let rows = sqlx::query_as::<_, SearchRow>(
        r#"
        SELECT e.event_id AS event_id,
               e.session_id AS session_id,
               e.node_seq AS node_seq,
               e.event_type AS event_type,
               e.created_at AS created_at,
               snippet(session_events_fts, 2, '[', ']', '…', 12) AS snippet
          FROM session_events_fts
          JOIN session_events e ON e.cursor = session_events_fts.rowid
         WHERE session_events_fts MATCH ?
         ORDER BY e.created_at DESC
         LIMIT ?
        "#,
    )
    .bind(match_query)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().filter_map(SearchRow::into_hit).collect())
}

/// LIKE fallback for search terms shorter than 3 characters.
async fn search_like(
    pool: &SqlitePool,
    needle: &str,
    limit: i64,
) -> Result<Vec<SearchHit>, DbError> {
    // The LIKE pattern also cannot be expressed with placeholders (wildcards must be
    // concatenated).
    let pattern = format!("%{}%", escape_like_pattern(needle));
    let rows = sqlx::query_as::<_, SearchRow>(
        r#"
        SELECT event_id, session_id, node_seq, event_type, created_at,
               searchable_text AS snippet
          FROM session_events
         WHERE searchable_text LIKE ? ESCAPE '\'
         ORDER BY created_at DESC
         LIMIT ?
        "#,
    )
    .bind(pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let text = row.snippet.as_deref()?;
            let snippet = excerpt_around(text, needle, SNIPPET_CONTEXT_CHARS);
            let node_seq = match u64::try_from(row.node_seq) {
                Ok(node_seq) => node_seq,
                Err(_) => return None,
            };
            Some(SearchHit {
                event_id: row.event_id,
                session_id: row.session_id,
                node_seq,
                event_type: row.event_type,
                snippet,
                created_at: row.created_at,
            })
        })
        .collect())
}

/// Retrieves the audit log list (newest first).
#[allow(
    clippy::manual_unwrap_or_default,
    reason = "a corrupt audit details column degrades to an empty object"
)]
pub async fn audit_logs(pool: &SqlitePool, limit: u32) -> Result<Vec<AuditLogEntry>, DbError> {
    let rows = sqlx::query_as!(
        AuditRow,
        r#"
        SELECT id, action AS "action: AuditAction",
               session_id AS "session_id: SessionId",
               node_id AS "node_id: NodeId", client_ip,
               client_user_agent,
               auth_subject, details_json, created_at
          FROM audit_logs
         ORDER BY created_at DESC, id DESC
         LIMIT ?
        "#,
        i64::from(limit.clamp(1, 1000)),
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| AuditLogEntry {
            id: row.id,
            action: row.action,
            session_id: row.session_id,
            node_id: row.node_id,
            client_ip: row.client_ip,
            client_user_agent: row.client_user_agent,
            auth_subject: row.auth_subject,
            details: match serde_json::from_str::<fxg_protocol::common::RawJson>(&row.details_json)
            {
                Ok(details) => details,
                Err(_) => fxg_protocol::common::RawJson::default(),
            },
            created_at: row.created_at,
        })
        .collect())
}

// ----------------------------------------------------------------------
// Row structs (for the type-safe sqlx macros)
// ----------------------------------------------------------------------

struct SessionRow {
    session_id: SessionId,
    project_id: ProjectId,
    node_id: NodeId,
    local_path: String,
    git_branch: Option<String>,
    is_worktree: bool,
    agent_id: String,
    agent_session_id: Option<String>,
    parent_session_id: Option<SessionId>,
    title: String,
    status: SessionStatus,
    current_mode: Option<String>,
    permission_policy: PermissionPolicy,
    usage_json: Option<String>,
    last_node_seq: i64,
    fork_scope: ForkScope,
    created_at: i64,
    updated_at: i64,
    archived_at: Option<i64>,
}

impl SessionRow {
    fn into_summary(self) -> Result<SessionSummary, DbError> {
        Ok(SessionSummary {
            session_id: self.session_id,
            project_id: self.project_id,
            node_id: self.node_id,
            local_path: self.local_path,
            git_branch: self.git_branch,
            is_worktree: self.is_worktree,
            agent_id: self.agent_id,
            agent_session_id: self.agent_session_id,
            parent_session_id: self.parent_session_id,
            title: self.title,
            status: self.status,
            current_mode: self.current_mode,
            permission_policy: self.permission_policy,
            usage: self
                .usage_json
                .as_deref()
                .map(serde_json::from_str::<SessionUsage>)
                .transpose()
                .map_err(|err| DbError::InvalidEnumValue(err.to_string()))?,
            last_node_seq: u64::try_from(self.last_node_seq)
                .map_err(|_| DbError::NegativeNodeSeq(self.last_node_seq))?,
            fork_scope: self.fork_scope,
            created_at: self.created_at,
            updated_at: self.updated_at,
            archived_at: self.archived_at,
        })
    }
}

struct ProjectRow {
    project_id: ProjectId,
    name: String,
    canonical_git_url: Option<String>,
    identity_kind: ProjectIdentityKind,
    project_key: Option<String>,
    created_at: i64,
    updated_at: i64,
}

struct BindingRow {
    project_id: ProjectId,
    node_id: NodeId,
    local_path: String,
    is_worktree: bool,
    path_exists: bool,
    git_branch: Option<String>,
    last_used_at: i64,
}

struct NodeRow {
    node_id: NodeId,
    name: String,
    os: String,
    arch: String,
    version: String,
    installed_agents_json: String,
    is_ephemeral: bool,
    provisioner: Option<String>,
    lifecycle_status: NodeLifecycleStatus,
    is_online: bool,
    last_seen_at: i64,
}

struct PermissionRow {
    request_id: String,
    session_id: SessionId,
    node_id: NodeId,
    tool_name: String,
    summary: String,
    details_json: String,
    status: PermissionRequestStatus,
    created_at: i64,
    resolved_at: Option<i64>,
    resolved_by: Option<ResolvedBy>,
}

impl PermissionRow {
    fn into_entry(self) -> Result<PermissionRequestEntry, DbError> {
        let stored: StoredPermissionDetails =
            serde_json::from_str(&self.details_json).map_err(|err| {
                DbError::InvalidPayload(format!(
                    "permission_requests.details_json for {}: {err}",
                    self.request_id
                ))
            })?;
        Ok(PermissionRequestEntry {
            request_id: self.request_id,
            session_id: self.session_id,
            node_id: self.node_id,
            tool_name: self.tool_name,
            summary: self.summary,
            options: stored.options,
            details: stored.details,
            status: self.status,
            created_at: self.created_at,
            resolved_at: self.resolved_at,
            resolved_by: self.resolved_by,
        })
    }
}

struct ElicitationRow {
    elicitation_id: String,
    session_id: SessionId,
    node_id: NodeId,
    message: String,
    mode: ElicitationMode,
    schema_json: String,
    tool_call_id: Option<String>,
    status: ElicitationRequestStatus,
    content_json: String,
    created_at: i64,
    resolved_at: Option<i64>,
    resolved_by: Option<ResolvedBy>,
}

impl ElicitationRow {
    fn into_entry(self) -> Result<ElicitationRequestEntry, DbError> {
        let requested_schema: fxg_protocol::common::ElicitationSchema =
            serde_json::from_str(&self.schema_json).map_err(|err| {
                DbError::InvalidPayload(format!(
                    "elicitation_requests.schema_json for {}: {err}",
                    self.elicitation_id
                ))
            })?;
        let content: Option<fxg_protocol::common::ElicitationContent> =
            serde_json::from_str(&self.content_json).map_err(|err| {
                DbError::InvalidPayload(format!(
                    "elicitation_requests.content_json for {}: {err}",
                    self.elicitation_id
                ))
            })?;
        Ok(ElicitationRequestEntry {
            elicitation_id: self.elicitation_id,
            session_id: self.session_id,
            node_id: self.node_id,
            message: self.message,
            mode: self.mode,
            requested_schema,
            tool_call_id: self.tool_call_id,
            content,
            status: self.status,
            created_at: self.created_at,
            resolved_at: self.resolved_at,
            resolved_by: self.resolved_by,
        })
    }
}

#[derive(sqlx::FromRow)]
struct SearchRow {
    event_id: EventId,
    session_id: SessionId,
    node_seq: i64,
    event_type: EventType,
    created_at: i64,
    snippet: Option<String>,
}

impl SearchRow {
    fn into_hit(self) -> Option<SearchHit> {
        let node_seq = match u64::try_from(self.node_seq) {
            Ok(node_seq) => node_seq,
            Err(_) => return None,
        };
        Some(SearchHit {
            event_id: self.event_id,
            session_id: self.session_id,
            node_seq,
            event_type: self.event_type,
            snippet: self.snippet?,
            created_at: self.created_at,
        })
    }
}

struct AuditRow {
    id: i64,
    action: AuditAction,
    session_id: Option<SessionId>,
    node_id: Option<NodeId>,
    client_ip: String,
    client_user_agent: Option<String>,
    auth_subject: String,
    details_json: String,
    created_at: i64,
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

/// Escapes LIKE pattern metacharacters (`%` / `_` / `\`).
///
/// Use together with LIKE specifying `ESCAPE '\'`.
fn escape_like_pattern(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len());
    for ch in needle.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Extracts `context_chars` characters before and after the match position to build a snippet.
///
/// If no match is found, cuts from the beginning (a fallback for languages whose length changes
/// under case folding, etc.). Byte positions always snap to character boundaries.
fn excerpt_around(text: &str, needle: &str, context_chars: usize) -> String {
    let lower_text = text.to_lowercase();
    let lower_needle = needle.to_lowercase();

    let Some(found) = lower_text.find(&lower_needle) else {
        return text.chars().take(context_chars * 2).collect();
    };
    let found_end = (found + needle.len()).min(text.len());

    let mut start = found.min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = found_end;
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }

    let before: String = text[..start]
        .chars()
        .rev()
        .take(context_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after: String = text[end..].chars().take(context_chars).collect();

    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.push_str(&before);
    snippet.push_str(&text[start..end]);
    snippet.push_str(&after);
    if end < text.len() {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_like_pattern_escapes_metacharacters() {
        assert_eq!(escape_like_pattern("50%_off"), "50\\%\\_off");
        assert_eq!(escape_like_pattern("back\\slash"), "back\\\\slash");
        assert_eq!(escape_like_pattern("plain"), "plain");
    }

    #[test]
    fn excerpt_around_centers_on_match() {
        let text = "0123456789あいうえおかきくけこabcdefghij"; // allow-japanese: multibyte snippet boundary test
        let snippet = excerpt_around(text, "あいうえお", 4); // allow-japanese: multibyte snippet boundary test
        assert_eq!(snippet, "…6789あいうえおかきくけ…"); // allow-japanese: multibyte snippet boundary test
    }

    #[test]
    fn excerpt_around_handles_missing_match() {
        let snippet = excerpt_around("abcdef", "zzz", 2);
        assert_eq!(snippet, "abcd");
    }

    #[test]
    fn excerpt_around_keeps_multibyte_intact() {
        let text = "こんにちは世界、これはテストです"; // allow-japanese: multibyte snippet test
        let snippet = excerpt_around(text, "世界", 3); // allow-japanese: multibyte snippet test
        assert!(snippet.contains("世界")); // allow-japanese: multibyte snippet test
        assert!(!snippet.is_empty());
    }
}
