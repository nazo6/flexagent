//! Audit log recording (`audit_logs`).
//!
//! Each store records only the operations it observed itself (not subject to sync).
//! Operations routed through the central server are recorded in `server.db`, while
//! operations made directly against the local node (CLI / `localhost:7860`) are also
//! recorded in the executing node's `node.db`
//! (design: `docs/01-architecture-and-sync.md` §7.5).

#![allow(
    clippy::disallowed_types,
    reason = "audit details are display-only opaque JSON; see docs/06-type-safety.md"
)]

use fxg_protocol::util::now_ms;
use sqlx::SqlitePool;

use crate::error::DbError;

/// Audit log `action` kinds.
pub mod actions {
    /// Session started
    pub const SESSION_START: &str = "session_start";
    /// Session reverted (Shadow Git Tree Revert)
    pub const SESSION_REVERT: &str = "session_revert";
    /// Session resumed (resume a stopped session)
    pub const SESSION_RESUME: &str = "session_resume";
    /// Session archived/restored
    pub const SESSION_ARCHIVE: &str = "session_archive";
    /// Session deleted (purge of event bodies)
    pub const SESSION_DELETE: &str = "session_delete";
    /// Permission resolved (Approve / Reject)
    pub const PERMISSION_RESOLVED: &str = "permission_resolved";

    /// Elicitation (question) resolved
    pub const ELICITATION_RESOLVED: &str = "elicitation_resolved";
    /// Web PTY spawned
    pub const PTY_SPAWN: &str = "pty_spawn";
    /// Emergency kill switch executed
    pub const KILL_SWITCH: &str = "kill_switch";
    /// Worktree operation (create / delete)
    pub const WORKTREE_MANAGE: &str = "worktree_manage";
    /// Worktree cleanup (prune)
    pub const WORKTREE_PRUNE: &str = "worktree_prune";
    /// Project binding (manual Link / bulk Scan)
    pub const PROJECT_LINK: &str = "project_link";
    /// Agent management (install / update / delete)
    pub const AGENT_MANAGE: &str = "agent_manage";
    /// Node token issuance / revocation
    pub const NODE_TOKEN_MANAGE: &str = "node_token_manage";
    /// Client auth token regeneration
    pub const AUTH_TOKEN_ROTATE: &str = "auth_token_rotate";
}

/// Contents of an audit log record.
#[derive(Debug, Clone)]
pub struct AuditLogRecord {
    /// Operation kind (a constant from [`actions`])
    pub action: String,
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
    pub details: serde_json::Value,
}

impl AuditLogRecord {
    /// Builds a record from the required fields (details default to an empty object).
    pub fn new(
        action: impl Into<String>,
        client_ip: impl Into<String>,
        auth_subject: impl Into<String>,
    ) -> Self {
        Self {
            action: action.into(),
            session_id: None,
            node_id: None,
            client_ip: client_ip.into(),
            client_user_agent: None,
            auth_subject: auth_subject.into(),
            details: serde_json::Value::Object(serde_json::Map::new()),
        }
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
        record.action,
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
    Ok(result.last_insert_rowid())
}
