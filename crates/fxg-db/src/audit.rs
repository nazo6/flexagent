//! Audit log recording (`audit_logs`).
//!
//! Each store records only the operations it observed itself (not subject to sync).
//! Operations routed through the hub are recorded in `hub.db`, while
//! operations made directly against the local node (CLI / `localhost:7860`) are also
//! recorded in the executing node's `node.db`
//! (design: `docs/01-architecture-and-sync.md` §7.5).

use std::sync::atomic::{AtomicU64, Ordering};

use fxg_protocol::common::{AuditAction, RawJson};
use fxg_protocol::util::now_ms;
use sqlx::SqlitePool;

use crate::error::DbError;

/// Audit rows older than this are pruned.
pub const AUDIT_RETENTION_DAYS: i64 = 90;
/// Upper bound on retained audit rows.
pub const AUDIT_MAX_ENTRIES: i64 = 100_000;
/// Prune at most once per this many appends (plus the first append).
const AUDIT_PRUNE_INTERVAL: u64 = 1_000;
static APPEND_COUNT: AtomicU64 = AtomicU64::new(0);

/// Audit log `action` kinds (kept for call-site readability).
pub mod actions {
    use fxg_protocol::common::AuditAction;

    /// Session started
    pub const SESSION_START: AuditAction = AuditAction::SessionStart;
    /// Session resumed (resume a stopped session)
    pub const SESSION_RESUME: AuditAction = AuditAction::SessionResume;
    /// Session archived/restored
    pub const SESSION_ARCHIVE: AuditAction = AuditAction::SessionArchive;
    /// Session deleted (purge of event bodies)
    pub const SESSION_DELETE: AuditAction = AuditAction::SessionDelete;
    /// Session renamed
    pub const SESSION_RENAME: AuditAction = AuditAction::SessionRename;
    /// Permission resolved (Approve / Reject)
    pub const PERMISSION_RESOLVED: AuditAction = AuditAction::PermissionResolved;
    /// Elicitation (question) resolved
    pub const ELICITATION_RESOLVED: AuditAction = AuditAction::ElicitationResolved;
    /// Web PTY spawned
    pub const PTY_SPAWN: AuditAction = AuditAction::PtySpawn;
    /// Web PTY terminated by a client
    pub const PTY_KILL: AuditAction = AuditAction::PtyKill;
    /// Emergency kill switch executed
    pub const KILL_SWITCH: AuditAction = AuditAction::KillSwitch;
    /// Worktree operation (create / delete)
    pub const WORKTREE_MANAGE: AuditAction = AuditAction::WorktreeManage;
    /// Worktree cleanup (prune)
    pub const WORKTREE_PRUNE: AuditAction = AuditAction::WorktreePrune;
    /// Workspace Git commit created
    pub const WORKSPACE_GIT_COMMIT: AuditAction = AuditAction::WorkspaceGitCommit;
    /// Workspace Git pull (fast-forward)
    pub const WORKSPACE_GIT_PULL: AuditAction = AuditAction::WorkspaceGitPull;
    /// Workspace Git push
    pub const WORKSPACE_GIT_PUSH: AuditAction = AuditAction::WorkspaceGitPush;
    /// Project binding (manual Link / bulk Scan)
    pub const PROJECT_LINK: AuditAction = AuditAction::ProjectLink;
    /// Agent management (install / update / delete)
    pub const AGENT_MANAGE: AuditAction = AuditAction::AgentManage;
    /// Node pairing token issuance / revocation
    pub const NODE_TOKEN_MANAGE: AuditAction = AuditAction::NodeTokenManage;
    /// Web access token regeneration
    pub const AUTH_TOKEN_ROTATE: AuditAction = AuditAction::AuthTokenRotate;
    /// Settings updated
    pub const SETTINGS_UPDATE: AuditAction = AuditAction::SettingsUpdate;
    /// Hub pairing updated / cleared
    pub const PAIRING_UPDATE: AuditAction = AuditAction::PairingUpdate;
    /// Secret created / updated / deleted / rotated
    pub const SECRET_MANAGE: AuditAction = AuditAction::SecretManage;
    /// Node shut down through the hub
    pub const NODE_SHUTDOWN: AuditAction = AuditAction::NodeShutdown;
}

/// Contents of an audit log record.
#[derive(Debug, Clone)]
pub struct AuditLogRecord {
    /// Operation kind
    pub action: AuditAction,
    /// Related session ID (optional)
    pub session_id: Option<String>,
    /// Target node ID (optional; on the node side this is the node itself)
    pub node_id: Option<String>,
    /// Source IP address
    pub client_ip: String,
    /// Client User-Agent
    pub client_user_agent: Option<String>,
    /// Token identifier or authenticated subject
    pub auth_subject: String,
    /// Operation details (executed command, approval option, etc.)
    pub details: RawJson,
}

impl AuditLogRecord {
    /// Builds a record from the required fields (details default to an empty object).
    pub fn new(
        action: AuditAction,
        client_ip: impl Into<String>,
        auth_subject: impl Into<String>,
    ) -> Self {
        Self {
            action,
            session_id: None,
            node_id: None,
            client_ip: client_ip.into(),
            client_user_agent: None,
            auth_subject: auth_subject.into(),
            details: RawJson::default(),
        }
    }

    /// Sets the related session.
    pub fn with_session(mut self, session_id: Option<&str>) -> Self {
        self.session_id = session_id.map(str::to_owned);
        self
    }

    /// Sets the target node (the node itself when recorded on the node side).
    pub fn with_node_id(mut self, node_id: Option<&str>) -> Self {
        self.node_id = node_id.map(str::to_owned);
        self
    }

    /// Sets the client User-Agent.
    pub fn with_user_agent(mut self, user_agent: Option<&str>) -> Self {
        self.client_user_agent = user_agent.map(str::to_owned);
        self
    }

    /// Sets the operation details.
    pub fn with_details(mut self, details: impl Into<RawJson>) -> Self {
        self.details = details.into();
        self
    }
}

/// Records a single audit log entry and returns the assigned `id`.
pub async fn append_audit_log(pool: &SqlitePool, record: &AuditLogRecord) -> Result<i64, DbError> {
    let details_json = serde_json::to_string(&record.details)?;
    let result = sqlx::query!(
        r#"
        INSERT INTO audit_logs
            (action, session_id, node_id, client_ip, client_user_agent, auth_subject,
             details_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
        record.action.as_str(),
        record.session_id.as_deref(),
        record.node_id.as_deref(),
        record.client_ip,
        record.client_user_agent.as_deref(),
        record.auth_subject,
        details_json,
        now_ms(),
    )
    .execute(pool)
    .await?;
    if APPEND_COUNT
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(AUDIT_PRUNE_INTERVAL)
        && let Err(err) = prune(pool).await
    {
        tracing::warn!("failed to prune the audit log: {err}");
    }
    Ok(result.last_insert_rowid())
}

/// Applies the audit retention period and the maximum row count.
pub async fn prune(pool: &SqlitePool) -> Result<u64, DbError> {
    let cutoff = now_ms() - AUDIT_RETENTION_DAYS * 24 * 60 * 60 * 1000;
    let expired = sqlx::query!(r#"DELETE FROM audit_logs WHERE created_at < ?"#, cutoff,)
        .execute(pool)
        .await?
        .rows_affected();
    let overflow = sqlx::query!(
        r#"
        DELETE FROM audit_logs
         WHERE id NOT IN (
             SELECT id
               FROM audit_logs
              ORDER BY created_at DESC, id DESC
              LIMIT ?
         )
        "#,
        AUDIT_MAX_ENTRIES,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(expired + overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Db, Role};

    #[tokio::test]
    async fn prune_removes_rows_older_than_the_retention_window() {
        let db = Db::open_in_memory(Role::Hub).await.expect("db");
        let old = now_ms() - (AUDIT_RETENTION_DAYS + 1) * 24 * 60 * 60 * 1000;
        sqlx::query!(
            r#"
            INSERT INTO audit_logs
                (action, session_id, node_id, client_ip, client_user_agent,
                 auth_subject, details_json, created_at)
            VALUES ('session_start', NULL, NULL, '127.0.0.1', NULL, 'local', '{}', ?)
            "#,
            old,
        )
        .execute(db.pool())
        .await
        .expect("insert old row");

        let removed = prune(db.pool()).await.expect("prune");
        assert_eq!(removed, 1);
        let remaining = sqlx::query_scalar!(r#"SELECT COUNT(*) FROM audit_logs"#)
            .fetch_one(db.pool())
            .await
            .expect("count");
        assert_eq!(remaining, 0);
    }
}
