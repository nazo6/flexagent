//! Watermark-based outbox extraction and ACK watermark updates.
//!
//! "Sent" is not represented by a per-row flag but by a per-session watermark,
//! `sessions.synced_up_to_node_seq` (the highest `node_seq` ACKed by the hub).
//! Because `node_seq` is a gap-free sequence allocated only by the executing node,
//! outbox extraction becomes the simple predicate `node_seq > synced_up_to_node_seq`,
//! and the update on ACK is O(1)
//! (design: `docs/01-architecture-and-sync.md` §2.2, `docs/02-database-schema.md` §0.4).

use fxg_protocol::events::SessionEventEnvelope;
use sqlx::SqlitePool;

use crate::error::DbError;
use crate::events::envelope_from_row;

/// Unsent events grouped by session.
///
/// `EventBatchPush` sends events split by session and ACKs are returned per session,
/// so extraction results are also batched per session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionOutboxBatch {
    /// Target session ID
    pub session_id: String,
    /// Unsent events (ascending by `node_seq`)
    pub events: Vec<SessionEventEnvelope>,
}

impl SessionOutboxBatch {
    /// Highest `node_seq` in the batch (used to validate the ACK watermark).
    pub fn last_node_seq(&self) -> Option<u64> {
        self.events.last().map(|event| event.node_seq)
    }
}

/// Extracts unsent events grouped by session.
pub async fn extract_outbox(pool: &SqlitePool) -> Result<Vec<SessionOutboxBatch>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT e.event_id, e.session_id, e.node_seq, e.payload_json, e.created_at
          FROM session_events e
          JOIN sessions s ON s.session_id = e.session_id
         WHERE e.node_seq > s.synced_up_to_node_seq
         ORDER BY e.session_id ASC, e.node_seq ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut batches: Vec<SessionOutboxBatch> = Vec::new();
    for row in rows {
        let envelope = envelope_from_row(
            row.event_id,
            row.session_id,
            row.node_seq,
            row.payload_json,
            row.created_at,
        )?;
        match batches.last_mut() {
            Some(batch) if batch.session_id == envelope.session_id => batch.events.push(envelope),
            _ => batches.push(SessionOutboxBatch {
                session_id: envelope.session_id.clone(),
                events: vec![envelope],
            }),
        }
    }
    Ok(batches)
}

/// For `ResyncRequest`: extracts events of the given session from `from_node_seq`
/// onward (`>=`) regardless of the watermark (`from_node_seq = 1` resends everything
/// including `SessionCreated`).
pub async fn extract_outbox_after(
    pool: &SqlitePool,
    session_id: &str,
    from_node_seq: u64,
) -> Result<Vec<SessionEventEnvelope>, DbError> {
    let from = i64::try_from(from_node_seq).map_err(|_| DbError::NodeSeqOutOfRange {
        session_id: session_id.to_owned(),
        node_seq: from_node_seq,
    })?;
    let rows = sqlx::query!(
        r#"
        SELECT event_id, session_id, node_seq, payload_json, created_at
          FROM session_events
         WHERE session_id = ? AND node_seq >= ?
         ORDER BY node_seq ASC
        "#,
        session_id,
        from,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            envelope_from_row(
                row.event_id,
                row.session_id,
                row.node_seq,
                row.payload_json,
                row.created_at,
            )
        })
        .collect()
}

/// Watermark update upon receiving `EventBatchAck`.
///
/// The watermark is monotonic (`MAX`) so that a delayed old ACK cannot rewind it.
pub async fn ack_watermark(
    pool: &SqlitePool,
    session_id: &str,
    acked_up_to_node_seq: u64,
) -> Result<(), DbError> {
    let acked = i64::try_from(acked_up_to_node_seq).map_err(|_| DbError::NodeSeqOutOfRange {
        session_id: session_id.to_owned(),
        node_seq: acked_up_to_node_seq,
    })?;
    let result = sqlx::query!(
        r#"
        UPDATE sessions
           SET synced_up_to_node_seq = MAX(synced_up_to_node_seq, ?)
         WHERE session_id = ?
        "#,
        acked,
        session_id,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::SessionNotFound(session_id.to_owned()));
    }
    Ok(())
}

/// Number of unsynced outbox events (for the sync status badge in the UI).
pub async fn unsynced_event_count(pool: &SqlitePool) -> Result<u64, DbError> {
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*)
          FROM session_events e
          JOIN sessions s ON s.session_id = e.session_id
         WHERE e.node_seq > s.synced_up_to_node_seq
        "#,
    )
    .fetch_one(pool)
    .await?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// `created_at` of the last ACKed event (last sync time).
///
/// `None` when nothing has been synced yet.
pub async fn last_synced_at(pool: &SqlitePool) -> Result<Option<i64>, DbError> {
    let value = sqlx::query_scalar!(
        r#"
        SELECT MAX(e.created_at)
          FROM session_events e
          JOIN sessions s ON s.session_id = e.session_id
         WHERE e.node_seq <= s.synced_up_to_node_seq
        "#,
    )
    .fetch_one(pool)
    .await?;
    Ok(value)
}
