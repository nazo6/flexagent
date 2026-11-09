//! Audit log recording (`audit_logs`).
//!
//! Each store records only the operations it observed itself (not subject to sync).
//! Operations routed through the central server are recorded in `server.db`, while
//! operations made directly against the local node (CLI / `localhost:7860`) are also
//! recorded in the executing node's `node.db`
//! (design: `docs/01-architecture-and-sync.md` §7.5).

use fxg_protocol::common::{AuditAction, RawJson};
use fxg_protocol::util::now_ms;
use sqlx::SqlitePool;

use crate::error::DbError;

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
    /// Permission resolved (Approve / Reject)
    pub const PERMISSION_RESOLVED: AuditAction = AuditAction::PermissionResolved;
    /// Elicitation (question) resolved
    pub const ELICITATION_RESOLVED: AuditAction = AuditAction::ElicitationResolved;
    /// Web PTY spawned
    pub const PTY_SPAWN: AuditAction = AuditAction::PtySpawn;
    /// Emergency kill switch executed
    pub const KILL_SWITCH: AuditAction = AuditAction::KillSwitch;
    /// Worktree operation (create / delete)
    pub const WORKTREE_MANAGE: AuditAction = AuditAction::WorktreeManage;
    /// Worktree cleanup (prune)
    pub const WORKTREE_PRUNE: AuditAction = AuditAction::WorktreePrune;
    /// Project binding (manual Link / bulk Scan)
    pub const PROJECT_LINK: AuditAction = AuditAction::ProjectLink;
    /// Agent management (install / update / delete)
    pub const AGENT_MANAGE: AuditAction = AuditAction::AgentManage;
    /// Node token issuance / revocation
    pub const NODE_TOKEN_MANAGE: AuditAction = AuditAction::NodeTokenManage;
    /// Client auth token regeneration
    pub const AUTH_TOKEN_ROTATE: AuditAction = AuditAction::AuthTokenRotate;
    /// Settings updated
    pub const SETTINGS_UPDATE: AuditAction = AuditAction::SettingsUpdate;
    /// Secret created / updated / deleted / rotated
    pub const SECRET_MANAGE: AuditAction = AuditAction::SecretManage;
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
    Ok(result.last_insert_rowid())
}
