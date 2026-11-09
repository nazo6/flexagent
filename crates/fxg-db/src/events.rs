//! Event application engine (idempotent append + projection update) and projection rebuild.
//!
//! Design: `docs/02-database-schema.md` §0.3 (event application and projections) / §2 (source
//! data and derived columns).
//!
//! - `append_events`: idempotent append via `INSERT OR IGNORE` plus projection updates for
//!   `sessions` / `permission_requests` / `elicitation_requests` in the **same transaction**
//! - `rebuild_projections`: rebuild projections from the full event log
//!   (tests guarantee that "state after applying events == state after full rebuild")
//! - `regenerate_searchable_text` / `rebuild_fts_index`: regeneration of FTS5 search data
//!
//! # Idempotency
//!
//! The `event_id` UNIQUE / `UNIQUE(session_id, node_seq)` constraints on `session_events`
//! cause duplicate events to be discarded by `INSERT OR IGNORE`. Projections are only updated
//! when a row was actually inserted, so resending the same batch (resend before ACK, resync
//! resend) converges to the same state no matter how many times it is applied.

#![allow(
    clippy::map_err_ignore,
    reason = "integer conversion errors (TryFromIntError) carry no additional context"
)]

use fxg_protocol::common::{
    ElicitationAction, ElicitationRequestStatus, PermissionOption, PermissionRequestStatus,
    RawJson, SessionStatus, SessionUsage,
};
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};

use crate::error::DbError;
use crate::searchable;

/// Application result for a single event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Event ID
    pub event_id: String,
    /// Target session ID
    pub session_id: String,
    /// In-session sequence number
    pub node_seq: u64,
    /// Whether the event was newly appended (`false` means discarded as a duplicate)
    pub inserted: bool,
    /// `session_events.cursor` of this event
    /// (ingest order in the source store; returned to clients as the incremental resume cursor)
    pub cursor: u64,
}

/// Result of sequence-numbered append ([`append_next_event`]).
#[derive(Debug, Clone, PartialEq)]
pub struct AppendedEvent {
    /// The appended event (node_seq / event_id already assigned)
    pub envelope: SessionEventEnvelope,
    /// `session_events.cursor` of this event
    pub cursor: u64,
}

/// Structured details stored in `permission_requests.details_json`.
///
/// Holds both `details` and `options` of the `PermissionRequest` event, used for rendering the
/// approval inbox and for determining the state of `PermissionResolved`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct StoredPermissionDetails {
    /// Argument and diff details
    #[serde(default)]
    pub details: RawJson,
    /// Options (option_id / name / kind)
    #[serde(default)]
    pub options: Vec<PermissionOption>,
}

/// Appends a sequence of events idempotently and updates projections in the same transaction.
///
/// Callers must pass the events in a batch ordered by ascending `node_seq`
/// (so that `SessionCreated` for an unknown session (`node_seq = 1`) is applied first and
/// satisfies the FK constraint to `sessions`).
pub async fn append_events(
    pool: &SqlitePool,
    events: &[SessionEventEnvelope],
) -> Result<Vec<ApplyOutcome>, DbError> {
    let mut tx = pool.begin().await?;
    let mut outcomes = Vec::with_capacity(events.len());
    for event in events {
        outcomes.push(append_event_in_tx(&mut tx, event).await?);
    }
    tx.commit().await?;
    Ok(outcomes)
}

/// Allocates the session's next `node_seq` and appends the event (**executing node only**).
///
/// Because `sessions.last_node_seq + 1` is allocated within the same transaction, concurrent
/// execution cannot produce gaps or duplicates. Events that must not be persisted
/// ([`UnifiedEventPayload::is_persistable`] returns `false`) are rejected.
///
/// # Errors
///
/// [`DbError::SessionNotFound`] if the target session does not exist.
pub async fn append_next_event(
    pool: &SqlitePool,
    session_id: &str,
    payload: UnifiedEventPayload,
) -> Result<AppendedEvent, DbError> {
    let mut tx = pool.begin().await?;

    // Allocation must always happen through this UPDATE, and the event append must complete
    // within the same transaction (on rollback, last_node_seq is also rewound, so no gaps
    // remain).
    // Appends to deleted sessions (tombstone applied) are rejected
    // (treated as `SessionNotFound`).
    let next_node_seq = sqlx::query_scalar!(
        r#"
        UPDATE sessions
           SET last_node_seq = last_node_seq + 1
         WHERE session_id = ? AND deleted_at IS NULL
        RETURNING last_node_seq
        "#,
        session_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(next_node_seq) = next_node_seq else {
        return Err(DbError::SessionNotFound(session_id.to_owned()));
    };
    let node_seq =
        u64::try_from(next_node_seq).map_err(|_| DbError::NegativeNodeSeq(next_node_seq))?;

    let envelope = SessionEventEnvelope {
        event_id: fxg_protocol::util::uuid_v7().into(),
        session_id: session_id.to_owned().into(),
        node_seq,
        created_at: fxg_protocol::util::now_ms(),
        payload,
    };
    let outcome = append_event_in_tx(&mut tx, &envelope).await?;
    debug_assert!(
        outcome.inserted,
        "newly allocated node_seq must be inserted"
    );
    tx.commit().await?;
    Ok(AppendedEvent {
        envelope,
        cursor: outcome.cursor,
    })
}

async fn append_event_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    envelope: &SessionEventEnvelope,
) -> Result<ApplyOutcome, DbError> {
    if !envelope.payload.is_persistable() {
        return Err(DbError::NonPersistableEvent(envelope.payload.event_type()));
    }
    let node_seq = node_seq_to_i64(envelope.session_id.as_str(), envelope.node_seq)?;
    let (event_type, searchable_text) = searchable::classify_payload(&envelope.payload);
    let payload_json = serde_json::to_string(&envelope.payload)?;

    // SessionCreated must satisfy the `session_events.session_id -> sessions` FK, so prepare
    // the sessions row (and any unknown project row) **before** appending the event.
    // (Events in a batch are passed in ascending node_seq order, so the first event of an
    //  unknown session is always SessionCreated = node_seq 1.)
    //
    // A replayed SessionCreated (resync / duplicated batch) is discarded by `INSERT OR IGNORE`
    // below. Only prepare the row for a new event: `upsert_session_from_created` overwrites
    // `title` (and the identity columns) with creation-time values, which would roll back a
    // later `SessionTitleChanged` when the duplicate itself is ignored.
    if matches!(envelope.payload, UnifiedEventPayload::SessionCreated { .. }) {
        // Same lookup as the duplicate branch below (cursor of an already stored event).
        let already_stored = sqlx::query_scalar!(
            r#"
            SELECT cursor FROM session_events
             WHERE event_id = ? OR (session_id = ? AND node_seq = ?)
             LIMIT 1
            "#,
            envelope.event_id,
            envelope.session_id,
            node_seq,
        )
        .fetch_optional(&mut **tx)
        .await?
        .is_some();
        if !already_stored {
            upsert_session_from_created(tx, envelope).await?;
        }
    }

    // The append decision is embedded in the INSERT statement (splitting it into `SELECT` then
    // `INSERT` would upgrade a read snapshot to a write and trigger `SQLITE_BUSY_SNAPSHOT`
    // (517)).
    //
    // - For deleted (tombstone-applied) sessions, only `SessionDeleted` is appended
    //   (prevents purged conversation content from reappearing when resync resends race with
    //   the stop handler).
    // - Nothing is appended for unknown sessions (so resync-originated tombstones become a
    //   no-op instead of an FK violation).
    let is_delete_event = matches!(envelope.payload, UnifiedEventPayload::SessionDeleted {});
    let result = sqlx::query!(
        r#"
        INSERT OR IGNORE INTO session_events
            (event_id, session_id, node_seq, event_type, payload_json, searchable_text, created_at)
        SELECT ?, ?, ?, ?, ?, ?, ?
         WHERE EXISTS (
             SELECT 1
               FROM sessions
              WHERE session_id = ?
                AND (deleted_at IS NULL OR ? = 1)
         )
        "#,
        envelope.event_id,
        envelope.session_id,
        node_seq,
        event_type.as_str(),
        payload_json,
        searchable_text,
        envelope.created_at,
        envelope.session_id,
        i64::from(is_delete_event),
    )
    .execute(&mut **tx)
    .await?;

    let inserted = result.rows_affected() > 0;
    let cursor = if inserted {
        result.last_insert_rowid()
    } else {
        // If it is a duplicate (matching event_id / (session_id, node_seq)), return the cursor
        // of the existing row. If it was discarded as non-appendable (deleted / unknown
        // session), return 0.
        sqlx::query_scalar!(
            r#"
            SELECT cursor FROM session_events
             WHERE event_id = ? OR (session_id = ? AND node_seq = ?)
             LIMIT 1
            "#,
            envelope.event_id,
            envelope.session_id,
            node_seq,
        )
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(0)
    };
    let cursor = u64::try_from(cursor).map_err(|_| DbError::NegativeNodeSeq(cursor))?;

    if inserted {
        apply_projections(tx, envelope).await?;
    }

    Ok(ApplyOutcome {
        event_id: envelope.event_id.to_string(),
        session_id: envelope.session_id.to_string(),
        node_seq: envelope.node_seq,
        inserted,
        cursor,
    })
}

/// Upserts `projects` / `sessions` rows from a `SessionCreated` event.
///
/// Idempotent (converges to the same state for both replay during rebuild and duplicate
/// application).
async fn upsert_session_from_created(
    conn: &mut SqliteConnection,
    envelope: &SessionEventEnvelope,
) -> Result<(), DbError> {
    let UnifiedEventPayload::SessionCreated {
        node_id,
        project_id,
        project_name,
        local_path,
        git_branch,
        is_worktree,
        agent_id,
        parent_session_id,
        title,
        ..
    } = &envelope.payload
    else {
        return Ok(());
    };

    let node_seq = node_seq_to_i64(envelope.session_id.as_str(), envelope.node_seq)?;

    // Prefer the current binding of the node + local path so that a `project_key`
    // change is not rolled back by a replayed `SessionCreated` event.
    let binding_project_id: Option<String> = sqlx::query_scalar!(
        r#"
        SELECT project_id AS "project_id!"
          FROM project_node_bindings
         WHERE node_id = ? AND local_path = ?
         LIMIT 1
        "#,
        node_id.as_str(),
        local_path,
    )
    .fetch_optional(&mut *conn)
    .await?;
    let project_id = binding_project_id.as_deref().unwrap_or(project_id.as_str());

    // Prepare a row in case the project is unknown on the hub side
    // (on the node side the source data already exists when the project is resolved, so
    // DO NOTHING).
    sqlx::query!(
        r#"
        INSERT INTO projects (project_id, name, canonical_git_url, created_at, updated_at)
        VALUES (?, ?, NULL, ?, ?)
        ON CONFLICT(project_id) DO NOTHING
        "#,
        project_id,
        project_name,
        envelope.created_at,
        envelope.created_at,
    )
    .execute(&mut *conn)
    .await?;

    // Overwrite all columns to support rebuild (do not use INSERT OR REPLACE, since it would
    // delete the event log through ON DELETE CASCADE).
    sqlx::query!(
        r#"
        INSERT INTO sessions (
            session_id, project_id, node_id, local_path, git_branch, is_worktree,
            agent_id, agent_session_id, parent_session_id,
            title, status, current_mode, available_modes_json,
            available_commands_json, config_options_json,
            last_node_seq, synced_up_to_node_seq, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, NULL, '[]', '[]', '[]', ?, 0, ?, ?)
        ON CONFLICT(session_id) DO UPDATE SET
            project_id = excluded.project_id,
            node_id = excluded.node_id,
            local_path = excluded.local_path,
            git_branch = excluded.git_branch,
            is_worktree = excluded.is_worktree,
            agent_id = excluded.agent_id,
            parent_session_id = excluded.parent_session_id,
            title = excluded.title,
            created_at = excluded.created_at
        "#,
        envelope.session_id,
        project_id,
        node_id,
        local_path,
        git_branch.as_deref(),
        is_worktree,
        agent_id,
        parent_session_id
            .as_ref()
            .map(fxg_protocol::SessionId::as_str),
        title,
        SessionStatus::Idle.as_str(),
        node_seq,
        envelope.created_at,
        envelope.created_at,
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Applies the projection updates for a single event (does not append to the event log).
///
/// Shared routine called from both [`append_events`] and [`rebuild_projections`].
pub(crate) async fn apply_projections(
    conn: &mut SqliteConnection,
    envelope: &SessionEventEnvelope,
) -> Result<(), DbError> {
    let node_seq = node_seq_to_i64(envelope.session_id.as_str(), envelope.node_seq)?;

    match &envelope.payload {
        UnifiedEventPayload::SessionCreated { .. } => {
            upsert_session_from_created(conn, envelope).await?;
        }
        UnifiedEventPayload::SessionTitleChanged { title } => {
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET title = ?, updated_at = MAX(updated_at, ?), last_node_seq = MAX(last_node_seq, ?)
                 WHERE session_id = ?
                "#,
                title,
                envelope.created_at,
                node_seq,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::SessionAgentBound { agent_session_id } => {
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET agent_session_id = ?, updated_at = MAX(updated_at, ?), last_node_seq = MAX(last_node_seq, ?)
                 WHERE session_id = ?
                "#,
                agent_session_id,
                envelope.created_at,
                node_seq,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::StatusChanged { status, .. } => {
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET status = ?, updated_at = MAX(updated_at, ?), last_node_seq = MAX(last_node_seq, ?)
                 WHERE session_id = ?
                "#,
                status.as_str(),
                envelope.created_at,
                node_seq,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            available_commands,
            config_options,
            fork_supported,
        } => {
            let available_modes_json = if available_modes.is_empty() {
                None
            } else {
                Some(serde_json::to_string(available_modes)?)
            };
            let available_commands_json = if available_commands.is_empty() {
                None
            } else {
                Some(serde_json::to_string(available_commands)?)
            };
            let config_options_json = if config_options.is_empty() {
                None
            } else {
                Some(serde_json::to_string(config_options)?)
            };
            // Keep existing values when current_mode / available_modes / available_commands /
            // config_options are unspecified or empty arrays
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET current_mode = COALESCE(?, current_mode),
                       available_modes_json = COALESCE(?, available_modes_json),
                       available_commands_json = COALESCE(?, available_commands_json),
                       config_options_json = COALESCE(?, config_options_json),
                       fork_supported = COALESCE(?, fork_supported),
                       updated_at = MAX(updated_at, ?),
                       last_node_seq = MAX(last_node_seq, ?)
                 WHERE session_id = ?
                "#,
                current_mode.as_deref(),
                available_modes_json,
                available_commands_json,
                config_options_json,
                *fork_supported,
                envelope.created_at,
                node_seq,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::PermissionRequest {
            request_id,
            tool_name,
            summary,
            options,
            details,
        } => {
            let details_json = serde_json::to_string(&StoredPermissionDetails {
                details: details.clone(),
                options: options.clone(),
            })?;
            sqlx::query!(
                r#"
                INSERT INTO permission_requests
                    (request_id, session_id, node_id, tool_name, summary, details_json,
                     status, created_at, resolved_at, resolved_by)
                SELECT ?, ?, s.node_id, ?, ?, ?, ?, ?, NULL, NULL
                  FROM sessions s
                 WHERE s.session_id = ?
                ON CONFLICT(request_id) DO UPDATE SET
                    tool_name = excluded.tool_name,
                    summary = excluded.summary,
                    details_json = excluded.details_json,
                    status = excluded.status,
                    resolved_at = NULL,
                    resolved_by = NULL
                "#,
                request_id,
                envelope.session_id,
                tool_name,
                summary,
                details_json,
                PermissionRequestStatus::Pending.as_str(),
                envelope.created_at,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::PermissionResolved {
            request_id,
            selected_option_id,
            resolved_by,
        } => {
            let stored: Option<String> = sqlx::query_scalar!(
                r#"SELECT details_json FROM permission_requests WHERE request_id = ?"#,
                request_id,
            )
            .fetch_optional(&mut *conn)
            .await?;
            let status = resolve_permission_status(stored.as_deref(), selected_option_id);
            // If there is no matching PermissionRequest row (e.g. resync started from a
            // mid-stream node_seq), this updates 0 rows (discarded).
            sqlx::query!(
                r#"
                UPDATE permission_requests
                   SET status = ?, resolved_by = ?, resolved_at = ?
                 WHERE request_id = ?
                "#,
                status.as_str(),
                resolved_by.as_str(),
                envelope.created_at,
                request_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::ElicitationRequest {
            elicitation_id,
            message,
            mode,
            requested_schema,
            tool_call_id,
        } => {
            let schema_json = serde_json::to_string(requested_schema)?;
            sqlx::query!(
                r#"
                INSERT INTO elicitation_requests
                    (elicitation_id, session_id, node_id, message, mode, schema_json,
                     tool_call_id, status, content_json, created_at, resolved_at, resolved_by)
                SELECT ?, ?, s.node_id, ?, ?, ?, ?, ?, 'null', ?, NULL, NULL
                  FROM sessions s
                 WHERE s.session_id = ?
                ON CONFLICT(elicitation_id) DO UPDATE SET
                    message = excluded.message,
                    mode = excluded.mode,
                    schema_json = excluded.schema_json,
                    tool_call_id = excluded.tool_call_id,
                    status = excluded.status,
                    content_json = 'null',
                    resolved_at = NULL,
                    resolved_by = NULL
                "#,
                elicitation_id,
                envelope.session_id,
                message,
                mode.as_str(),
                schema_json,
                tool_call_id,
                ElicitationRequestStatus::Pending.as_str(),
                envelope.created_at,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::ElicitationResolved {
            elicitation_id,
            action,
            content,
            resolved_by,
        } => {
            let status = elicitation_status(*action);
            let content_json = serde_json::to_string(content)?;
            // If there is no matching ElicitationRequest row (e.g. resync started from a
            // mid-stream node_seq), this updates 0 rows (discarded).
            sqlx::query!(
                r#"
                UPDATE elicitation_requests
                   SET status = ?, content_json = ?, resolved_by = ?, resolved_at = ?
                 WHERE elicitation_id = ?
                "#,
                status.as_str(),
                content_json,
                resolved_by.as_str(),
                envelope.created_at,
                elicitation_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::UsageUpdated {
            used_tokens,
            context_size,
            cost,
        } => {
            let usage_json = serde_json::to_string(&SessionUsage {
                used_tokens: *used_tokens,
                context_size: *context_size,
                cost: cost.clone(),
            })?;
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET usage_json = ?
                 WHERE session_id = ?
                "#,
                usage_json,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::SessionArchived { archived } => {
            // Archiving is a reversible visibility flag (the event log is retained)
            let archived_at = archived.then_some(envelope.created_at);
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET archived_at = ?
                 WHERE session_id = ?
                "#,
                archived_at,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::SessionPermissionPolicyChanged { policy } => {
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET permission_policy = ?
                 WHERE session_id = ?
                "#,
                policy.as_str(),
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        UnifiedEventPayload::SessionDeleted {} => {
            // Apply tombstone: keep the row itself so that deletion propagation through
            // Outbox / Resync and references to the fork source stay consistent, and
            // physically delete body events and approval history.
            sqlx::query!(
                r#"
                UPDATE sessions
                   SET deleted_at = ?, archived_at = NULL
                 WHERE session_id = ?
                "#,
                envelope.created_at,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
            // Keep this event (the tombstone) and purge earlier bodies
            sqlx::query!(
                r#"DELETE FROM session_events WHERE session_id = ? AND node_seq < ?"#,
                envelope.session_id,
                node_seq,
            )
            .execute(&mut *conn)
            .await?;
            sqlx::query!(
                r#"DELETE FROM permission_requests WHERE session_id = ?"#,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
            sqlx::query!(
                r#"DELETE FROM elicitation_requests WHERE session_id = ?"#,
                envelope.session_id,
            )
            .execute(&mut *conn)
            .await?;
        }
        // Events without derived-column updates on the sessions row
        // (conversation, tool execution, terminal output, BootstrapLog, etc.).
        // Only the shared watermark / update-time update is applied.
        UnifiedEventPayload::UserMessage { .. }
        | UnifiedEventPayload::AgentMessage { .. }
        | UnifiedEventPayload::AgentThought { .. }
        | UnifiedEventPayload::ToolCall { .. }
        | UnifiedEventPayload::PlanUpdate { .. }
        | UnifiedEventPayload::TerminalOutput { .. }
        | UnifiedEventPayload::TerminalInput { .. }
        | UnifiedEventPayload::BootstrapLog { .. }
        | UnifiedEventPayload::TurnEnded { .. }
        | UnifiedEventPayload::CompactionUpdated { .. } => {}
    }

    // Common to all events: reflect the highest applied node_seq and the update time in the
    // projections
    sqlx::query!(
        r#"
        UPDATE sessions
           SET last_node_seq = MAX(last_node_seq, ?), updated_at = MAX(updated_at, ?)
         WHERE session_id = ?
        "#,
        node_seq,
        envelope.created_at,
        envelope.session_id,
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Converts an elicitation response action into the DB projection state.
fn elicitation_status(action: ElicitationAction) -> ElicitationRequestStatus {
    match action {
        ElicitationAction::Accept => ElicitationRequestStatus::Accepted,
        ElicitationAction::Decline => ElicitationRequestStatus::Declined,
        ElicitationAction::Cancel => ElicitationRequestStatus::Cancelled,
    }
}

/// Determines the resolution state from the kind of the option matching `selected_option_id`.
///
/// Refers to the `options` stored in `details_json`: `reject*` kinds map to `rejected`, all
/// others to `approved`. If the options cannot be retrieved (e.g. mid-resync), falls back to
/// the `option_id` naming convention (containing `reject` / `deny`).
#[allow(
    clippy::disallowed_methods,
    reason = "mid-resync rows may lack decodable details; the documented fallback uses the option id naming"
)]
fn resolve_permission_status(
    details_json: Option<&str>,
    selected_option_id: &str,
) -> PermissionRequestStatus {
    let kind = details_json
        .and_then(|json| serde_json::from_str::<StoredPermissionDetails>(json).ok())
        .and_then(|stored| {
            stored
                .options
                .into_iter()
                .find(|option| option.option_id == selected_option_id)
        })
        .map(|option| option.kind);

    match kind {
        Some(kind) if kind.is_reject() => PermissionRequestStatus::Rejected,
        Some(_) => PermissionRequestStatus::Approved,
        None => {
            if option_id_looks_like_reject(selected_option_id) {
                PermissionRequestStatus::Rejected
            } else {
                PermissionRequestStatus::Approved
            }
        }
    }
}

/// Fallback for mid-resync rows without decodable options.
///
/// Option IDs conventionally contain `reject` / `deny` for rejection.
#[allow(
    clippy::disallowed_methods,
    reason = "documented legacy fallback for rows whose option kind is unavailable"
)]
fn option_id_looks_like_reject(selected_option_id: &str) -> bool {
    selected_option_id.contains("reject") || selected_option_id.contains("deny")
}

/// Rebuilds the `sessions` / `permission_requests` / `elicitation_requests` projections from
/// the full event log.
///
/// # Implementation constraints
///
/// - `session_events.session_id` references `sessions` with `ON DELETE CASCADE`, so `sessions`
///   rows cannot be DELETEd (it would also wipe the event log).
///   The procedure is therefore "reset derived columns to defaults → replay from
///   `SessionCreated` in cursor order".
/// - `synced_up_to_node_seq` (node-specific watermark) is preserved because it
///   does not come from events.
/// - The entire event log is loaded into memory (a simplification assuming self-hosted,
///   single-tenant scale).
pub async fn rebuild_projections(pool: &SqlitePool) -> Result<(), DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT event_id, session_id, node_seq, payload_json, created_at
          FROM session_events
         ORDER BY cursor ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut tx = pool.begin().await?;

    sqlx::query!(r#"DELETE FROM permission_requests"#)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(r#"DELETE FROM elicitation_requests"#)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        r#"
        UPDATE sessions
           SET title = 'New Session',
               status = ?,
               current_mode = NULL,
               permission_policy = 'ask',
               available_modes_json = '[]',
               available_commands_json = '[]',
               config_options_json = '[]',
               usage_json = NULL,
               agent_session_id = NULL,
               fork_supported = 0,
               archived_at = NULL,
               deleted_at = NULL,
               last_node_seq = 0,
               updated_at = created_at
        "#,
        SessionStatus::Idle.as_str(),
    )
    .execute(&mut *tx)
    .await?;

    for row in rows {
        let envelope = envelope_from_row(
            row.event_id,
            row.session_id,
            row.node_seq,
            row.payload_json,
            row.created_at,
        )?;
        apply_projections(&mut tx, &envelope).await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Regenerates `searchable_text` / `event_type` from payloads
/// (for recomputation after changing the extraction logic).
///
/// The `session_events` UPDATE is also reflected in the search index through the FTS5 sync
/// trigger (`session_events_fts_au`).
pub async fn regenerate_searchable_text(pool: &SqlitePool) -> Result<u64, DbError> {
    let rows = sqlx::query!(
        r#"SELECT cursor, event_id, session_id, node_seq, payload_json, created_at FROM session_events"#,
    )
    .fetch_all(pool)
    .await?;

    let mut tx = pool.begin().await?;
    let mut updated_rows = 0u64;
    for row in rows {
        let envelope = envelope_from_row(
            row.event_id,
            row.session_id,
            row.node_seq,
            row.payload_json,
            row.created_at,
        )?;
        let (event_type, searchable_text) = searchable::classify_payload(&envelope.payload);
        let result = sqlx::query!(
            r#"UPDATE session_events SET event_type = ?, searchable_text = ? WHERE cursor = ?"#,
            event_type.as_str(),
            searchable_text,
            row.cursor,
        )
        .execute(&mut *tx)
        .await?;
        updated_rows += result.rows_affected();
    }
    tx.commit().await?;
    Ok(updated_rows)
}

/// Rebuilds the FTS5 external content index from the content table.
///
/// Since `searchable_text` is re-read from the content table (`session_events`), run
/// [`regenerate_searchable_text`] beforehand.
pub async fn rebuild_fts_index(pool: &SqlitePool) -> Result<(), DbError> {
    // The FTS5 'rebuild' command has a special syntax with no placeholders, so the type-safe
    // sqlx macro (`query!`) cannot be used (a documented exception, covered by unit tests).
    sqlx::query(r#"INSERT INTO session_events_fts(session_events_fts) VALUES('rebuild')"#)
        .execute(pool)
        .await?;
    Ok(())
}

/// Restores a [`SessionEventEnvelope`] from a DB row (`payload_json`).
pub(crate) fn envelope_from_row(
    event_id: String,
    session_id: String,
    node_seq: i64,
    payload_json: String,
    created_at: i64,
) -> Result<SessionEventEnvelope, DbError> {
    let payload: UnifiedEventPayload = serde_json::from_str(&payload_json)
        .map_err(|err| DbError::InvalidPayload(format!("event {event_id}: {err}")))?;
    let node_seq = u64::try_from(node_seq).map_err(|_| DbError::NegativeNodeSeq(node_seq))?;
    Ok(SessionEventEnvelope {
        event_id: event_id.into(),
        session_id: session_id.into(),
        node_seq,
        created_at,
        payload,
    })
}

fn node_seq_to_i64(session_id: &str, node_seq: u64) -> Result<i64, DbError> {
    i64::try_from(node_seq).map_err(|_| DbError::NodeSeqOutOfRange {
        session_id: session_id.to_owned(),
        node_seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::common::PermissionOptionKind;

    fn details(options: &[(&str, PermissionOptionKind)]) -> String {
        let options = options
            .iter()
            .map(|(option_id, kind)| PermissionOption {
                option_id: (*option_id).to_owned(),
                name: (*option_id).to_owned(),
                kind: kind.clone(),
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&StoredPermissionDetails {
            details: serde_json::json!({}).into(),
            options,
        })
        .expect("details")
    }

    #[test]
    fn permission_status_uses_the_matching_option_kind() {
        let stored = details(&[
            ("allow", PermissionOptionKind::AllowOnce),
            ("deny", PermissionOptionKind::RejectOnce),
        ]);
        assert_eq!(
            resolve_permission_status(Some(&stored), "allow"),
            PermissionRequestStatus::Approved
        );
        assert_eq!(
            resolve_permission_status(Some(&stored), "deny"),
            PermissionRequestStatus::Rejected
        );
    }

    #[test]
    fn permission_status_falls_back_to_the_option_id() {
        assert_eq!(
            resolve_permission_status(None, "reject_once"),
            PermissionRequestStatus::Rejected
        );
        assert_eq!(
            resolve_permission_status(None, "deny"),
            PermissionRequestStatus::Rejected
        );
        assert_eq!(
            resolve_permission_status(None, "allow_once"),
            PermissionRequestStatus::Approved
        );
    }
}
