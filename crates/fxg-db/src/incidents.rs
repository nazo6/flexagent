//! Diagnostic incidents (`incidents` table).
//!
//! Incidents are a bounded, grouped history of failures and degradations. Both
//! `node.db` and `server.db` record the errors they observed themselves; the
//! table is not part of session sync (design:
//! `docs/changelog/2026-11-08-error-handling-and-recovery-plan.md` Phase 3).
//!
//! Grouping keeps a reconnect loop from filling the table: while an incident is
//! unresolved, another occurrence with the same `(code, source, session_id,
//! node_id)` inside [`GROUPING_WINDOW_MS`] increments `occurrence_count` instead
//! of inserting a new row.

use std::path::Path;
use std::str::FromStr;

use fxg_protocol::common::{
    ErrorCode, IncidentEntry, IncidentLevel, IncidentSource, RawJson, SupportBundleSession,
};
use fxg_protocol::util::now_ms;
use fxg_protocol::{NodeId, SessionId};
use sqlx::SqlitePool;

use crate::error::DbError;

/// Days an incident is retained.
pub const INCIDENT_RETENTION_DAYS: u32 = 14;
/// Maximum number of retained incident rows (ring buffer).
pub const INCIDENT_MAX_ENTRIES: u32 = 1000;
/// Window in which the same cause is grouped into one incident.
pub const GROUPING_WINDOW_MS: i64 = 10 * 60 * 1000;

/// Input of [`record`].
#[derive(Debug, Clone)]
pub struct IncidentRecord {
    /// Severity
    pub level: IncidentLevel,
    /// Structured error code
    pub code: ErrorCode,
    /// English human-readable summary
    pub summary: String,
    /// Technical details
    pub details: RawJson,
    /// Related session
    pub session_id: Option<String>,
    /// Related node
    pub node_id: Option<String>,
    /// Request correlation ID
    pub request_id: Option<String>,
    /// Component that produced the incident
    pub source: IncidentSource,
}

impl IncidentRecord {
    /// Builds a record with no session / node / request context.
    pub fn new(
        level: IncidentLevel,
        code: ErrorCode,
        source: IncidentSource,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            level,
            code,
            summary: summary.into(),
            details: RawJson::default(),
            session_id: None,
            node_id: None,
            request_id: None,
            source,
        }
    }

    /// Attaches the related session.
    pub fn with_session(mut self, session_id: Option<&str>) -> Self {
        self.session_id = session_id.map(str::to_owned);
        self
    }

    /// Attaches the related node.
    pub fn with_node_id(mut self, node_id: Option<&str>) -> Self {
        self.node_id = node_id.map(str::to_owned);
        self
    }

    /// Attaches the request correlation ID.
    pub fn with_request_id(mut self, request_id: Option<&str>) -> Self {
        self.request_id = request_id.map(str::to_owned);
        self
    }

    /// Attaches technical details.
    pub fn with_details(mut self, details: impl Into<RawJson>) -> Self {
        self.details = details.into();
        self
    }
}

#[derive(sqlx::FromRow)]
struct IncidentRow {
    id: i64,
    created_at: i64,
    last_seen_at: i64,
    level: String,
    code: String,
    summary: String,
    details_json: String,
    session_id: Option<String>,
    node_id: Option<String>,
    request_id: Option<String>,
    source: String,
    occurrence_count: i64,
    resolved_at: Option<i64>,
}

impl IncidentRow {
    fn into_entry(self) -> Result<IncidentEntry, DbError> {
        let level = IncidentLevel::from_str(&self.level)
            .map_err(|err| DbError::InvalidEnumValue(format!("incident level: {err}")))?;
        let source = IncidentSource::from_str(&self.source)
            .map_err(|err| DbError::InvalidEnumValue(format!("incident source: {err}")))?;
        let code = ErrorCode::from_str(&self.code)
            .map_err(|err| DbError::InvalidEnumValue(format!("incident code: {err}")))?;
        let details: RawJson = serde_json::from_str(&self.details_json)?;
        Ok(IncidentEntry {
            id: self.id,
            created_at: self.created_at,
            last_seen_at: self.last_seen_at,
            level,
            code,
            summary: self.summary,
            details,
            session_id: self.session_id.map(SessionId::from),
            node_id: self.node_id.map(NodeId::from),
            request_id: self.request_id,
            source,
            occurrence_count: u64::try_from(self.occurrence_count).map_err(|err| {
                DbError::InvalidPayload(format!("negative incident occurrence count: {err}"))
            })?,
            resolved_at: self.resolved_at,
        })
    }
}

/// Records an incident, grouping it with a recent unresolved entry of the same cause.
pub async fn record(pool: &SqlitePool, record: &IncidentRecord) -> Result<IncidentEntry, DbError> {
    let now = now_ms();
    let details_json = serde_json::to_string(&record.details)?;

    // `session_id IS ?` / `node_id IS ?` compare NULLs correctly in SQLite.
    let existing = sqlx::query!(
        r#"
        SELECT id
          FROM incidents
         WHERE code = ?
           AND source = ?
           AND session_id IS ?
           AND node_id IS ?
           AND resolved_at IS NULL
           AND last_seen_at >= ?
         ORDER BY last_seen_at DESC
         LIMIT 1
        "#,
        record.code.as_str(),
        record.source.as_str(),
        record.session_id.as_deref(),
        record.node_id.as_deref(),
        now - GROUPING_WINDOW_MS,
    )
    .fetch_optional(pool)
    .await?;

    let id = if let Some(row) = existing {
        sqlx::query!(
            r#"
            UPDATE incidents
               SET last_seen_at = ?,
                   occurrence_count = occurrence_count + 1,
                   level = ?,
                   summary = ?,
                   details_json = ?,
                   request_id = COALESCE(?, request_id)
             WHERE id = ?
            "#,
            now,
            record.level.as_str(),
            record.summary,
            details_json,
            record.request_id.as_deref(),
            row.id,
        )
        .execute(pool)
        .await?;
        row.id
    } else {
        let result = sqlx::query!(
            r#"
            INSERT INTO incidents
                (created_at, last_seen_at, level, code, summary, details_json,
                 session_id, node_id, request_id, source, occurrence_count, resolved_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, NULL)
            "#,
            now,
            now,
            record.level.as_str(),
            record.code.as_str(),
            record.summary,
            details_json,
            record.session_id.as_deref(),
            record.node_id.as_deref(),
            record.request_id.as_deref(),
            record.source.as_str(),
        )
        .execute(pool)
        .await?;
        result.last_insert_rowid()
    };

    prune(pool).await?;
    get(pool, id).await
}

/// Fetches one incident by ID.
pub async fn get(pool: &SqlitePool, id: i64) -> Result<IncidentEntry, DbError> {
    let row = sqlx::query_as!(
        IncidentRow,
        r#"
        SELECT id, created_at, last_seen_at, level, code, summary, details_json,
               session_id, node_id, request_id, source, occurrence_count, resolved_at
          FROM incidents
         WHERE id = ?
        "#,
        id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::IncidentNotFound(id))?;
    row.into_entry()
}

/// Lists incidents newest first.
pub async fn list(
    pool: &SqlitePool,
    limit: u32,
    include_resolved: bool,
) -> Result<Vec<IncidentEntry>, DbError> {
    let rows = sqlx::query_as!(
        IncidentRow,
        r#"
        SELECT id, created_at, last_seen_at, level, code, summary, details_json,
               session_id, node_id, request_id, source, occurrence_count, resolved_at
          FROM incidents
         WHERE ? OR resolved_at IS NULL
         ORDER BY last_seen_at DESC
         LIMIT ?
        "#,
        include_resolved,
        i64::from(limit),
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(IncidentRow::into_entry).collect()
}

/// Marks one incident as resolved. Returns whether a row changed.
pub async fn resolve(pool: &SqlitePool, id: i64) -> Result<bool, DbError> {
    let now = now_ms();
    let result = sqlx::query!(
        r#"
        UPDATE incidents
           SET resolved_at = COALESCE(resolved_at, ?)
         WHERE id = ?
        "#,
        now,
        id,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Resolves every unresolved incident with the given code and source.
pub async fn resolve_code(
    pool: &SqlitePool,
    code: ErrorCode,
    source: IncidentSource,
) -> Result<u64, DbError> {
    let now = now_ms();
    let result = sqlx::query!(
        r#"
        UPDATE incidents
           SET resolved_at = ?
         WHERE code = ?
           AND source = ?
           AND resolved_at IS NULL
        "#,
        now,
        code.as_str(),
        source.as_str(),
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Applies the retention period and the maximum row count.
pub async fn prune(pool: &SqlitePool) -> Result<u64, DbError> {
    let now = now_ms();
    let cutoff = now - i64::from(INCIDENT_RETENTION_DAYS) * 24 * 60 * 60 * 1000;
    let expired = sqlx::query!(
        r#"
        DELETE FROM incidents
         WHERE last_seen_at < ?
        "#,
        cutoff,
    )
    .execute(pool)
    .await?
    .rows_affected();
    let overflow = sqlx::query!(
        r#"
        DELETE FROM incidents
         WHERE id NOT IN (
             SELECT id
               FROM incidents
              ORDER BY last_seen_at DESC
              LIMIT ?
         )
        "#,
        i64::from(INCIDENT_MAX_ENTRIES),
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(expired + overflow)
}

/// Returns unresolved incidents for health aggregation (newest first).
pub async fn unresolved_since(
    pool: &SqlitePool,
    since: i64,
) -> Result<Vec<IncidentEntry>, DbError> {
    let rows = sqlx::query_as!(
        IncidentRow,
        r#"
        SELECT id, created_at, last_seen_at, level, code, summary, details_json,
               session_id, node_id, request_id, source, occurrence_count, resolved_at
          FROM incidents
         WHERE resolved_at IS NULL
           AND last_seen_at >= ?
         ORDER BY last_seen_at DESC
         LIMIT ?
        "#,
        since,
        i64::from(INCIDENT_MAX_ENTRIES),
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(IncidentRow::into_entry).collect()
}

/// Counters / sizes used by diagnostics and the support bundle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DatabaseCounts {
    /// Number of sessions
    pub sessions: u64,
    /// Number of persisted events
    pub events: u64,
    /// Number of incidents
    pub incidents: u64,
}

/// Returns row counts used by diagnostics.
pub async fn counts(pool: &SqlitePool) -> Result<DatabaseCounts, DbError> {
    let row = sqlx::query!(
        r#"
        SELECT (SELECT COUNT(*) FROM sessions) AS sessions,
               (SELECT COUNT(*) FROM session_events) AS events,
               (SELECT COUNT(*) FROM incidents) AS incidents
        "#
    )
    .fetch_one(pool)
    .await?;
    Ok(DatabaseCounts {
        sessions: u64::try_from(row.sessions)
            .map_err(|err| DbError::InvalidPayload(format!("negative session count: {err}")))?,
        events: u64::try_from(row.events)
            .map_err(|err| DbError::InvalidPayload(format!("negative event count: {err}")))?,
        incidents: u64::try_from(row.incidents)
            .map_err(|err| DbError::InvalidPayload(format!("negative incident count: {err}")))?,
    })
}

/// Returns the file size in bytes (`0` when the path does not exist).
pub fn file_size(path: &Path) -> u64 {
    match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(err) => {
            tracing::debug!(path = %path.display(), "failed to read database metadata: {err}");
            0
        }
    }
}

/// Number of persisted events of one session.
pub async fn event_count(pool: &SqlitePool, session_id: &str) -> Result<u64, DbError> {
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM session_events WHERE session_id = ?"#,
        session_id,
    )
    .fetch_one(pool)
    .await?;
    u64::try_from(count)
        .map_err(|err| DbError::InvalidPayload(format!("negative event count: {err}")))
}

/// Returns recent sessions with their event counts for a support bundle.
pub async fn support_bundle_sessions(
    pool: &SqlitePool,
    limit: u32,
) -> Result<Vec<SupportBundleSession>, DbError> {
    let filter = crate::queries::SessionFilter {
        include_archived: true,
        limit: Some(limit),
        ..crate::queries::SessionFilter::default()
    };
    let summaries = crate::queries::list_sessions(pool, &filter).await?;
    let mut sessions = Vec::with_capacity(summaries.len());
    for session in summaries {
        let event_count = event_count(pool, session.session_id.as_str()).await?;
        sessions.push(SupportBundleSession {
            session,
            event_count,
        });
    }
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, DbRole};

    fn record(summary: &str) -> IncidentRecord {
        IncidentRecord::new(
            IncidentLevel::Error,
            ErrorCode::GitFailed,
            IncidentSource::Git,
            summary,
        )
        .with_session(Some("ses-1"))
        .with_node_id(Some("node-1"))
    }

    async fn db() -> Db {
        Db::open_in_memory(DbRole::Node).await.expect("db")
    }

    #[tokio::test]
    async fn groups_repeated_occurrences() {
        let db = db().await;
        let one = db
            .record_incident(&record("git diff failed"))
            .await
            .expect("record");
        let two = db
            .record_incident(&record("git diff failed again"))
            .await
            .expect("record");
        assert_eq!(one.id, two.id);
        assert_eq!(two.occurrence_count, 2);
        assert_eq!(two.summary, "git diff failed again");
        assert!(two.last_seen_at >= two.created_at);
    }

    #[tokio::test]
    async fn resolved_incident_starts_a_new_group() {
        let db = db().await;
        let first = db.record_incident(&record("first")).await.expect("record");
        assert!(db.resolve_incident(first.id).await.expect("resolve"));

        let second = db.record_incident(&record("second")).await.expect("record");
        assert_ne!(first.id, second.id);
        assert_eq!(second.occurrence_count, 1);

        let visible = db.list_incidents(10, false).await.expect("list");
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, second.id);
        let all = db.list_incidents(10, true).await.expect("list");
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn prunes_expired_rows() {
        let db = db().await;
        let expired = now_ms() - (i64::from(INCIDENT_RETENTION_DAYS) + 1) * 86_400_000;
        sqlx::query!(
            r#"
            INSERT INTO incidents
                (created_at, last_seen_at, level, code, summary, source)
            VALUES (?, ?, 'error', 'GIT_FAILED', 'old', 'git')
            "#,
            expired,
            expired,
        )
        .execute(db.pool())
        .await
        .expect("insert old");
        db.record_incident(&record("fresh")).await.expect("record");
        let incidents = db.list_incidents(10, true).await.expect("list");
        assert_eq!(incidents.len(), 1);
        assert_eq!(incidents[0].summary, "fresh");
    }

    #[tokio::test]
    async fn counts_and_event_counts_are_available() {
        let db = db().await;
        let counts = db.database_counts().await.expect("counts");
        assert_eq!(counts.incidents, 0);
        db.record_incident(&record("x")).await.expect("record");
        let counts = db.database_counts().await.expect("counts");
        assert_eq!(counts.incidents, 1);
    }
}
