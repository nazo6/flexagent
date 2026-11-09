//! SQLite connection management and migration application.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fxg_protocol::common::{
    AuditLogEntry, ElicitationRequestEntry, NodeSummary, PermissionRequestEntry, ProjectSummary,
    SearchHit, SessionSummary,
};
use fxg_protocol::config::{NODE_DB_FILE_NAME, SERVER_DB_FILE_NAME};
use fxg_protocol::events::{SessionEventBatch, SessionEventEnvelope, UnifiedEventPayload};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error::DbError;
use crate::{audit, events, outbox, queries, registration};

/// Execution role of a DB.
///
/// `node.db` / `server.db` share the same schema, and behavioral differences branch on this
/// role (role-based branching appears in outbox sync, relaying, Web Push, etc. from Phase 4
/// onward).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbRole {
    /// Each node's local DB (`node.db`). The **authority** for session state.
    Node,
    /// Central server DB (`server.db`). Only updates **projections** via event application.
    Hub,
}

impl DbRole {
    /// Role name (`node` / `hub`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Hub => "hub",
        }
    }
}

/// Returns the path to `~/.flexagent/node.db`.
pub fn node_db_path(fxg_home: &Path) -> PathBuf {
    fxg_home.join(NODE_DB_FILE_NAME)
}

/// Returns the path to `~/.flexagent/server.db`.
pub fn hub_db_path(fxg_home: &Path) -> PathBuf {
    fxg_home.join(SERVER_DB_FILE_NAME)
}

/// SQLite DB handle (connection pool + execution role).
///
/// `SqlitePool` only clones a handle (Arc), so `Db` itself is also cheap to clone
/// (used for shared daemon state, etc.).
#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
    role: DbRole,
}

impl Db {
    /// Opens a file-backed DB. Creates the parent directory and applies migrations.
    ///
    /// WAL mode / `foreign_keys = ON` / `busy_timeout` are applied to every connection
    /// (`PRAGMA` inside migrations has no effect within a transaction).
    pub async fn open(path: &Path, role: DbRole) -> Result<Self, DbError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        Self::from_pool(pool, role).await
    }

    /// Opens an in-memory DB (for tests).
    ///
    /// Because in-memory DBs are independent per connection, the pool is pinned to a single
    /// connection.
    pub async fn open_in_memory(role: DbRole) -> Result<Self, DbError> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        Self::from_pool(pool, role).await
    }

    async fn from_pool(pool: SqlitePool, role: DbRole) -> Result<Self, DbError> {
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool, role })
    }

    /// Returns a reference to the connection pool (when using the shared query layer directly).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Returns the execution role.
    pub fn role(&self) -> DbRole {
        self.role
    }

    /// Closes the connection pool.
    pub async fn close(self) {
        self.pool.close().await;
    }

    // ------------------------------------------------------------------
    // Event application engine (crate::events)
    // ------------------------------------------------------------------

    /// Appends events idempotently and updates projections in the same transaction.
    pub async fn append_events(
        &self,
        events: &[SessionEventEnvelope],
    ) -> Result<Vec<events::ApplyOutcome>, DbError> {
        events::append_events(&self.pool, events).await
    }

    /// Appends a single event idempotently and updates projections.
    pub async fn append_event(
        &self,
        event: &SessionEventEnvelope,
    ) -> Result<events::ApplyOutcome, DbError> {
        events::append_events(&self.pool, std::slice::from_ref(event))
            .await
            .map(|mut outcomes| outcomes.remove(0))
    }

    /// Allocates the session's next `node_seq` and appends the event (executing node only).
    pub async fn append_next_event(
        &self,
        session_id: &str,
        payload: UnifiedEventPayload,
    ) -> Result<events::AppendedEvent, DbError> {
        events::append_next_event(&self.pool, session_id, payload).await
    }

    /// Rebuilds the `sessions` / `permission_requests` projections from the full event log.
    pub async fn rebuild_projections(&self) -> Result<(), DbError> {
        events::rebuild_projections(&self.pool).await
    }

    /// Regenerates `searchable_text` / `event_type` from the payloads.
    pub async fn regenerate_searchable_text(&self) -> Result<u64, DbError> {
        events::regenerate_searchable_text(&self.pool).await
    }

    /// Rebuilds the FTS5 external content index from the content table.
    pub async fn rebuild_fts_index(&self) -> Result<(), DbError> {
        events::rebuild_fts_index(&self.pool).await
    }

    // ------------------------------------------------------------------
    // Watermark-based outbox (crate::outbox)
    // ------------------------------------------------------------------

    /// Extracts unsent events grouped by session.
    pub async fn extract_outbox(&self) -> Result<Vec<outbox::SessionOutboxBatch>, DbError> {
        outbox::extract_outbox(&self.pool).await
    }

    /// For `ResyncRequest`: extracts the given session from `from_node_seq` onward regardless
    /// of the watermark.
    pub async fn extract_outbox_after(
        &self,
        session_id: &str,
        from_node_seq: u64,
    ) -> Result<Vec<SessionEventEnvelope>, DbError> {
        outbox::extract_outbox_after(&self.pool, session_id, from_node_seq).await
    }

    /// Watermark update upon receiving `EventBatchAck` (`synced_up_to_node_seq`).
    pub async fn ack_watermark(
        &self,
        session_id: &str,
        acked_up_to_node_seq: u64,
    ) -> Result<(), DbError> {
        outbox::ack_watermark(&self.pool, session_id, acked_up_to_node_seq).await
    }

    /// Number of unsynced outbox events (for the sync badge in the UI).
    pub async fn unsynced_event_count(&self) -> Result<u64, DbError> {
        outbox::unsynced_event_count(&self.pool).await
    }

    /// Time of the last ACKed event (last sync time).
    pub async fn last_synced_at(&self) -> Result<Option<i64>, DbError> {
        outbox::last_synced_at(&self.pool).await
    }

    // ------------------------------------------------------------------
    // Node self-registration and project binding (crate::registration)
    // ------------------------------------------------------------------

    /// Upserts node information (`token_hash` is preserved).
    pub async fn upsert_node(&self, record: &registration::NodeRecord) -> Result<(), DbError> {
        registration::upsert_node(&self.pool, record).await
    }

    /// Updates the node's online state.
    pub async fn set_node_online(&self, node_id: &str, is_online: bool) -> Result<(), DbError> {
        registration::set_node_online(&self.pool, node_id, is_online).await
    }

    /// Updates the node's lifecycle state (ephemeral VM provisioning / terminated, etc.).
    pub async fn set_node_lifecycle(
        &self,
        node_id: &str,
        status: fxg_protocol::common::NodeLifecycleStatus,
    ) -> Result<(), DbError> {
        registration::set_node_lifecycle(&self.pool, node_id, status).await
    }

    /// Updates the session state while an ephemeral VM is being provisioned (hub only).
    pub async fn set_provisional_session_status(
        &self,
        session_id: &str,
        status: fxg_protocol::common::SessionStatus,
    ) -> Result<(), DbError> {
        registration::set_provisional_session_status(&self.pool, session_id, status).await
    }

    /// Sets or revokes the hash of a per-node token.
    pub async fn set_node_token_hash(
        &self,
        node_id: &str,
        token_hash: Option<&str>,
    ) -> Result<(), DbError> {
        registration::set_node_token_hash(&self.pool, node_id, token_hash).await
    }

    /// Returns the list of per-node token hashes `(node_id, token_hash)`.
    pub async fn list_node_tokens(&self) -> Result<Vec<(String, String)>, DbError> {
        registration::list_node_tokens(&self.pool).await
    }

    /// Upserts a logical project.
    pub async fn upsert_project(
        &self,
        record: &registration::ProjectRecord,
    ) -> Result<(), DbError> {
        registration::upsert_project(&self.pool, record).await
    }

    /// Upserts a project × node local path binding.
    pub async fn upsert_project_binding(
        &self,
        record: &registration::ProjectBindingRecord,
    ) -> Result<(), DbError> {
        registration::upsert_project_binding(&self.pool, record).await
    }

    /// Deletes a project × node local path binding (returns the number of deleted rows).
    pub async fn delete_project_binding(
        &self,
        node_id: &str,
        local_path: &str,
    ) -> Result<u64, DbError> {
        registration::delete_project_binding(&self.pool, node_id, local_path).await
    }

    /// Deletes bindings of other projects that share the same `(node_id, local_path)`.
    pub async fn delete_other_project_bindings(
        &self,
        node_id: &str,
        local_path: &str,
        keep_project_id: &str,
    ) -> Result<u64, DbError> {
        registration::delete_other_project_bindings(
            &self.pool,
            node_id,
            local_path,
            keep_project_id,
        )
        .await
    }

    /// Deletes orphan projects that have neither bindings nor sessions.
    pub async fn delete_orphan_projects(&self) -> Result<u64, DbError> {
        registration::delete_orphan_projects(&self.pool).await
    }

    /// Inserts a provisional session row while an ephemeral VM is provisioning (hub only,
    /// idempotent).
    pub async fn upsert_provisional_session(
        &self,
        record: &registration::ProvisionalSessionRecord,
    ) -> Result<(), DbError> {
        registration::upsert_provisional_session(&self.pool, record).await
    }

    // ------------------------------------------------------------------
    // Shared client-facing queries (crate::queries)
    // ------------------------------------------------------------------

    /// Session list (descending by update time).
    pub async fn list_sessions(
        &self,
        filter: &queries::SessionFilter,
    ) -> Result<Vec<SessionSummary>, DbError> {
        queries::list_sessions(&self.pool, filter).await
    }

    /// Retrieves a single session.
    pub async fn get_session(&self, session_id: &str) -> Result<Option<SessionSummary>, DbError> {
        queries::get_session(&self.pool, session_id).await
    }

    /// Resolves the default execution directory path (binding first, then latest session).
    pub async fn resolve_default_local_path(
        &self,
        project_id: &fxg_protocol::ProjectId,
        node_id: &fxg_protocol::NodeId,
    ) -> Result<Option<String>, DbError> {
        queries::resolve_default_local_path(&self.pool, project_id, node_id).await
    }

    /// Retrieves the event history after `after_cursor` (batch for incremental resume).
    pub async fn session_events_after(
        &self,
        session_id: &str,
        after_cursor: u64,
        limit: u32,
    ) -> Result<SessionEventBatch, DbError> {
        queries::session_events_after(&self.pool, session_id, after_cursor, limit).await
    }

    /// Retrieves events after `after_cursor` across all sessions
    /// (for Client WS replay).
    pub async fn events_after_cursor(
        &self,
        after_cursor: u64,
        limit: u32,
    ) -> Result<SessionEventBatch, DbError> {
        queries::events_after_cursor(&self.pool, after_cursor, limit).await
    }

    /// Latest cursor of `session_events` (0 when there are no events).
    pub async fn latest_cursor(&self) -> Result<u64, DbError> {
        queries::latest_cursor(&self.pool).await
    }

    /// Logical project list (including node / Worktree bindings).
    pub async fn list_projects(&self) -> Result<Vec<ProjectSummary>, DbError> {
        queries::list_projects(&self.pool).await
    }

    /// Node list (including online state and ephemeral node attributes).
    pub async fn list_nodes(&self) -> Result<Vec<NodeSummary>, DbError> {
        queries::list_nodes(&self.pool).await
    }

    /// Unresolved (`pending`) permission requests (global approval inbox).
    pub async fn pending_permissions(&self) -> Result<Vec<PermissionRequestEntry>, DbError> {
        queries::pending_permissions(&self.pool).await
    }

    /// Retrieves a single permission request.
    pub async fn find_permission_request(
        &self,
        request_id: &str,
    ) -> Result<Option<PermissionRequestEntry>, DbError> {
        queries::find_permission_request(&self.pool, request_id).await
    }

    /// Unresolved (`pending`) elicitation requests (question inbox).
    pub async fn pending_elicitations(&self) -> Result<Vec<ElicitationRequestEntry>, DbError> {
        queries::pending_elicitations(&self.pool).await
    }

    /// Retrieves a single elicitation request.
    pub async fn find_elicitation_request(
        &self,
        elicitation_id: &str,
    ) -> Result<Option<ElicitationRequestEntry>, DbError> {
        queries::find_elicitation_request(&self.pool, elicitation_id).await
    }

    /// FTS5 full-text search (falls back to LIKE for queries shorter than 3 characters).
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>, DbError> {
        queries::search(&self.pool, query, limit).await
    }

    /// Audit log list (newest first).
    pub async fn audit_logs(&self, limit: u32) -> Result<Vec<AuditLogEntry>, DbError> {
        queries::audit_logs(&self.pool, limit).await
    }

    /// Records an audit log entry.
    pub async fn append_audit_log(&self, record: &audit::AuditLogRecord) -> Result<i64, DbError> {
        audit::append_audit_log(&self.pool, record).await
    }

    // ------------------------------------------------------------------
    // Web Push subscriptions (central server)
    // ------------------------------------------------------------------

    /// Registers or updates a Web Push subscription (same `endpoint` overwrites the keys).
    pub async fn upsert_push_subscription(
        &self,
        record: &crate::push::PushSubscriptionRecord,
    ) -> Result<(), DbError> {
        crate::push::upsert_push_subscription(&self.pool, record).await
    }

    /// Retrieves all Web Push subscriptions.
    pub async fn list_push_subscriptions(
        &self,
    ) -> Result<Vec<crate::push::PushSubscriptionRecord>, DbError> {
        crate::push::list_push_subscriptions(&self.pool).await
    }

    /// Deletes a Web Push subscription (cleanup of expired subscriptions).
    pub async fn delete_push_subscription(&self, endpoint: &str) -> Result<(), DbError> {
        crate::push::delete_push_subscription(&self.pool, endpoint).await
    }
}
