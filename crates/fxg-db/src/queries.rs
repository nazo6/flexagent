//! Shared client-facing query layer.
//!
//! Lets both `node.db` (local direct UI / CLI) and `hub.db` (hub UI) return
//! **the same response types** ([`SessionSummary`] / [`PermissionRequestEntry`], etc.)
//! (design: `docs/02-database-schema.md` §3).
//!
//! # Policy: SQL push-down for list filters
//!
//! List filters are applied in SQL. [`list_session_page`] builds its `WHERE` clause with
//! [`sqlx::QueryBuilder`] because the filter is optional and combinatorial; the row shape is
//! still a typed `SessionRow` (`FromRow`), and the column list is one constant shared by
//! every session-list query. Filtering in Rust would read every session row on each request
//! (O(total sessions) per page) and defeat the keyset indexes.
//!
//! The type-safe macros (`query!` / `query_as!` / `query_scalar!`) remain the default for
//! static SQL. The documented exceptions are:
//!
//! - the session-list filter (`QueryBuilder`, as above),
//! - FTS5 `MATCH` and the LIKE fallback for search terms shorter than 3 characters (special
//!   syntax that cannot go through placeholders, covered by unit tests).
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
    ResolvedBy, SearchHit, SessionCursor, SessionStatus, SessionSummary, SessionUsage,
};
use fxg_protocol::events::SessionEventBatch;
use fxg_protocol::node_hub::SessionSyncState;
use fxg_protocol::settings::ProjectIdentityKind;
use fxg_protocol::{EventId, NodeId, ProjectId, SessionId};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

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
    /// Return only sessions strictly older than this keyset cursor
    pub cursor: Option<SessionCursor>,
}

/// One page of the session list.
#[derive(Debug, Clone, Default)]
pub struct SessionPage {
    /// Sessions of this page (newest update first)
    pub sessions: Vec<SessionSummary>,
    /// Whether at least one more session matches the filter after this page
    pub has_more: bool,
    /// Per-project continuation for the projects present in `sessions`.
    ///
    /// Projects that are not part of this page have no entry (their
    /// continuation is unknown). A project with `next_cursor = None` has no
    /// older sessions.
    pub project_cursors: Vec<ProjectSessionCursor>,
}

/// Continuation of one project's session list ([`SessionPage::project_cursors`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSessionCursor {
    /// Logical project ID
    pub project_id: ProjectId,
    /// Cursor of the project's next (older) page (`None` when fully loaded)
    pub next_cursor: Option<SessionCursor>,
}

/// Context character count for LIKE search and snippet generation.
const SNIPPET_CONTEXT_CHARS: usize = 40;

/// Finds the live session that already owns an agent-side session.
///
/// Deleted (tombstone) sessions are ignored so a rolled-back / removed import is
/// offered again on the next discovery.
pub async fn find_session_by_agent_session(
    pool: &SqlitePool,
    agent_id: &str,
    agent_session_id: &str,
) -> Result<Option<String>, DbError> {
    let session_id = sqlx::query_scalar!(
        r#"
        SELECT session_id AS "session_id!"
          FROM sessions
         WHERE agent_id = ? AND agent_session_id = ? AND deleted_at IS NULL
         ORDER BY updated_at DESC
         LIMIT 1
        "#,
        agent_id,
        agent_session_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(session_id)
}

/// Column list of `SessionRow` (kept in sync with its fields).
const SESSION_ROW_COLUMNS: &str = "session_id, project_id, node_id, local_path, git_branch, \
     is_worktree, agent_id, agent_session_id, parent_session_id, title, status, \
     current_mode, permission_policy, usage_json, last_node_seq, fork_scope, \
     created_at, updated_at, archived_at";

/// Retrieves the session list (descending by update time).
///
/// Deleted (tombstone) sessions are always excluded. Archived sessions are excluded when
/// [`SessionFilter::include_archived`] is `false`.
///
/// Without [`SessionFilter::limit`] this reads every matching row; callers that only need
/// identifiers or counts should use the dedicated lightweight queries instead.
pub async fn list_sessions(
    pool: &SqlitePool,
    filter: &SessionFilter,
) -> Result<Vec<SessionSummary>, DbError> {
    Ok(fetch_session_page(pool, filter, false).await?.sessions)
}

/// Retrieves one keyset-paginated page of the session list.
///
/// Ordering is `updated_at` descending, then `session_id` ascending. The
/// filter's [`SessionFilter::cursor`] is the last entry of the previous page;
/// [`SessionPage::has_more`] tells the caller whether another page exists.
pub async fn list_session_page(
    pool: &SqlitePool,
    filter: &SessionFilter,
) -> Result<SessionPage, DbError> {
    fetch_session_page(pool, filter, true).await
}

async fn fetch_session_page(
    pool: &SqlitePool,
    filter: &SessionFilter,
    with_project_cursors: bool,
) -> Result<SessionPage, DbError> {
    let mut builder = QueryBuilder::<Sqlite>::new("SELECT ");
    builder.push(SESSION_ROW_COLUMNS);
    builder.push(" FROM sessions");
    push_session_filters(&mut builder, filter);
    builder.push(" ORDER BY updated_at DESC, session_id ASC");
    if let Some(limit) = filter.limit {
        // One extra row distinguishes "exactly at the limit" from "more rows exist".
        builder.push(" LIMIT ").push_bind(i64::from(limit) + 1);
    }
    let mut rows = builder
        .build_query_as::<SessionRow>()
        .fetch_all(pool)
        .await?;

    let limit = filter
        .limit
        .map(|limit| usize::try_from(limit).unwrap_or(usize::MAX));
    let has_more = limit.is_some_and(|limit| rows.len() > limit);
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
    let sessions = rows
        .into_iter()
        .map(SessionRow::into_summary)
        .collect::<Result<Vec<_>, _>>()?;
    let project_cursors = if with_project_cursors {
        project_cursors(pool, filter, &sessions, has_more).await?
    } else {
        Vec::new()
    };

    Ok(SessionPage {
        sessions,
        has_more,
        project_cursors,
    })
}

/// Computes the per-project continuation of one page.
///
/// A project present in `sessions` gets an entry. Its `next_cursor` is the last
/// session of that project in the page when the project has an older matching
/// session, and `None` when it is fully loaded. When the page already contains
/// every matching session (`has_more = false`), no project can have an older
/// session and the probes are skipped. Otherwise the check is one indexed
/// `EXISTS` probe per project (at most the page size), so the result is exact
/// without scanning the rest of the session table.
async fn project_cursors(
    pool: &SqlitePool,
    filter: &SessionFilter,
    sessions: &[SessionSummary],
    has_more: bool,
) -> Result<Vec<ProjectSessionCursor>, DbError> {
    let mut project_ids: Vec<&ProjectId> = Vec::new();
    for session in sessions {
        if !project_ids.contains(&&session.project_id) {
            project_ids.push(&session.project_id);
        }
    }

    let mut cursors = Vec::with_capacity(project_ids.len());
    for project_id in project_ids {
        let last = sessions
            .iter()
            .rev()
            .find(|session| &session.project_id == project_id)
            .expect("project is present in the page");
        let last_cursor = SessionCursor {
            updated_at: last.updated_at,
            session_id: last.session_id.clone(),
        };

        let has_older = if has_more {
            let mut probe = filter.clone();
            probe.project_id = Some(project_id.clone());
            probe.cursor = Some(last_cursor.clone());
            probe.limit = None;
            let mut builder = QueryBuilder::<Sqlite>::new("SELECT 1 FROM sessions");
            push_session_filters(&mut builder, &probe);
            builder.push(" LIMIT 1");
            builder
                .build_query_scalar::<i64>()
                .fetch_optional(pool)
                .await?
                .is_some()
        } else {
            false
        };

        cursors.push(ProjectSessionCursor {
            project_id: project_id.clone(),
            next_cursor: has_older.then_some(last_cursor),
        });
    }
    Ok(cursors)
}

/// Appends the `WHERE` conditions of `filter` to a session query.
///
/// Every session-list query shares this clause so the SQL push-down stays
/// consistent (and index-friendly) across the page, count and column queries.
fn push_session_filters(builder: &mut QueryBuilder<Sqlite>, filter: &SessionFilter) {
    builder.push(" WHERE deleted_at IS NULL");
    if let Some(project_id) = filter.project_id.as_ref() {
        builder
            .push(" AND project_id = ")
            .push_bind(project_id.as_str());
    }
    if let Some(node_id) = filter.node_id.as_ref() {
        builder.push(" AND node_id = ").push_bind(node_id.as_str());
    }
    if !filter.include_archived {
        builder.push(" AND archived_at IS NULL");
    }
    if !filter.statuses.is_empty() {
        builder.push(" AND status IN (");
        let mut statuses = builder.separated(", ");
        for status in &filter.statuses {
            statuses.push_bind(status.as_str());
        }
        statuses.push_unseparated(")");
    }
    if let Some(cursor) = filter.cursor.as_ref() {
        builder
            .push(" AND (updated_at < ")
            .push_bind(cursor.updated_at)
            .push(" OR (updated_at = ")
            .push_bind(cursor.updated_at)
            .push(" AND session_id > ")
            .push_bind(cursor.session_id.as_str())
            .push("))");
    }
}

/// Appends `filter.limit` as a plain `LIMIT` (no `has_more` look-ahead).
fn push_limit(builder: &mut QueryBuilder<Sqlite>, limit: Option<u32>) {
    if let Some(limit) = limit {
        builder.push(" LIMIT ").push_bind(i64::from(limit));
    }
}

/// Counts the sessions matching `filter`.
///
/// Used by callers that only need a number (the tray's active-session badge)
/// instead of loading every summary.
pub async fn count_sessions(pool: &SqlitePool, filter: &SessionFilter) -> Result<u64, DbError> {
    let mut builder = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM sessions");
    push_session_filters(&mut builder, filter);
    let count = builder.build_query_scalar::<i64>().fetch_one(pool).await?;
    u64::try_from(count)
        .map_err(|_| DbError::InvalidPayload(format!("negative session count: {count}")))
}

/// Lists the IDs of the sessions matching `filter` (order is unspecified).
///
/// Used by startup reconciliation and the kill switch, which only need the
/// session IDs to append a `StatusChanged` event.
pub async fn list_session_ids(
    pool: &SqlitePool,
    filter: &SessionFilter,
) -> Result<Vec<SessionId>, DbError> {
    let mut builder = QueryBuilder::<Sqlite>::new("SELECT session_id FROM sessions");
    push_session_filters(&mut builder, filter);
    push_limit(&mut builder, filter.limit);
    Ok(builder
        .build_query_scalar::<SessionId>()
        .fetch_all(pool)
        .await?)
}

/// Lists the `NodeHello` sync state of the sessions matching `filter`.
///
/// Selects only `(session_id, project_id, last_node_seq)` instead of loading
/// every summary column.
pub async fn list_session_sync_states(
    pool: &SqlitePool,
    filter: &SessionFilter,
) -> Result<Vec<SessionSyncState>, DbError> {
    let mut builder =
        QueryBuilder::<Sqlite>::new("SELECT session_id, project_id, last_node_seq FROM sessions");
    push_session_filters(&mut builder, filter);
    push_limit(&mut builder, filter.limit);
    let rows = builder
        .build_query_as::<SessionSyncRow>()
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(SessionSyncState {
                session_id: row.session_id,
                project_id: row.project_id,
                last_node_seq: u64::try_from(row.last_node_seq)
                    .map_err(|_| DbError::NegativeNodeSeq(row.last_node_seq))?,
            })
        })
        .collect()
}

/// Lists the `local_path` values of the sessions matching `filter`.
///
/// Used by workspace validation and project-link re-resolution, which only
/// compare paths.
pub async fn list_session_local_paths(
    pool: &SqlitePool,
    filter: &SessionFilter,
) -> Result<Vec<String>, DbError> {
    let mut builder = QueryBuilder::<Sqlite>::new("SELECT local_path FROM sessions");
    push_session_filters(&mut builder, filter);
    push_limit(&mut builder, filter.limit);
    Ok(builder
        .build_query_scalar::<String>()
        .fetch_all(pool)
        .await?)
}

/// Lists `(agent_id, agent_session_id, session_id)` of live sessions that are
/// bound to an agent-side session.
///
/// Used by external-session discovery to skip sessions that were already
/// imported.
pub async fn list_agent_session_bindings(
    pool: &SqlitePool,
) -> Result<Vec<(String, String, SessionId)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT agent_id, agent_session_id AS "agent_session_id!",
               session_id AS "session_id!: SessionId"
          FROM sessions
         WHERE deleted_at IS NULL AND agent_session_id IS NOT NULL
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.agent_id, row.agent_session_id, row.session_id))
        .collect())
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

/// Returns the node that owns a session, including tombstoned (deleted) rows.
///
/// The hub uses this to reject events sent by a node other than the session
/// owner. Unlike [`get_session`], deleted sessions are included so a replayed
/// `SessionDeleted` tombstone can still be attributed to its owner.
pub async fn session_owner(pool: &SqlitePool, session_id: &str) -> Result<Option<NodeId>, DbError> {
    let owner = sqlx::query_scalar!(
        r#"SELECT node_id AS "node_id: NodeId" FROM sessions WHERE session_id = ?"#,
        session_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(owner)
}

/// Number of persisted events of one session.
pub async fn session_event_count(pool: &SqlitePool, session_id: &str) -> Result<u64, DbError> {
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM session_events WHERE session_id = ?"#,
        session_id,
    )
    .fetch_one(pool)
    .await?;
    u64::try_from(count)
        .map_err(|err| DbError::InvalidPayload(format!("negative event count: {err}")))
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
    let mut first_cursor = 0;
    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let row_cursor =
            u64::try_from(row.cursor).map_err(|_| DbError::NegativeNodeSeq(row.cursor))?;
        if first_cursor == 0 {
            first_cursor = row_cursor;
        }
        cursor = row_cursor;
        events.push(envelope_from_row(
            row.event_id,
            row.session_id,
            row.node_seq,
            row.payload_json,
            row.created_at,
        )?);
    }
    Ok(SessionEventBatch {
        events,
        cursor,
        first_cursor,
    })
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
    let mut first_cursor = 0;
    let mut events = Vec::with_capacity(rows.len());
    for (row_cursor, event_id, session_id, node_seq, payload_json, created_at) in rows {
        let row_cursor =
            u64::try_from(row_cursor).map_err(|_| DbError::NegativeNodeSeq(row_cursor))?;
        if first_cursor == 0 {
            first_cursor = row_cursor;
        }
        cursor = row_cursor;
        events.push(envelope_from_row(
            event_id,
            session_id,
            node_seq,
            payload_json,
            created_at,
        )?);
    }
    Ok(SessionEventBatch {
        events,
        cursor,
        first_cursor,
    })
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
/// Returns the **most recently used** `(project_id, node_id)` binding, including Worktrees
/// (see [`select_default_binding_path`]). Sessions always register their binding when they
/// start, so the binding table is the only source of a default path.
///
/// Used as the default when `local_path` is omitted in a session start request, shared by both
/// the node and the hub (design: docs/01-architecture-and-sync.md §4).
pub async fn resolve_default_local_path(
    pool: &SqlitePool,
    project_id: &ProjectId,
    node_id: &NodeId,
) -> Result<Option<String>, DbError> {
    let projects = list_projects(pool).await?;
    Ok(projects
        .iter()
        .find(|project| &project.project_id == project_id)
        .and_then(|project| select_default_binding_path(&project.bindings, node_id)))
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
    session_id: &str,
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
         WHERE session_id = ? AND request_id = ?
        "#,
        session_id,
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
    session_id: &str,
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
         WHERE session_id = ? AND elicitation_id = ?
        "#,
        session_id,
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

#[derive(sqlx::FromRow)]
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

#[derive(sqlx::FromRow)]
struct SessionSyncRow {
    session_id: SessionId,
    project_id: ProjectId,
    last_node_seq: i64,
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
