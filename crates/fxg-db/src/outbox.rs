//! 水位ベース Outbox 抽出と ACK 水位更新。
//!
//! 「送信済み」は行単位フラグではなく、セッション単位の水位
//! `sessions.synced_up_to_node_seq` (ハブが ACK した最大 `node_seq`) で表現する。
//! `node_seq` は実行ノードだけが採番する欠番のない連番のため、Outbox の抽出は
//! `node_seq > synced_up_to_node_seq` の単純な述語になり、ACK 時の更新も O(1) になる
//! (設計: `docs/01-architecture-and-sync.md` §2.2, `docs/02-database-schema.md` §0.4)。

use fxg_protocol::events::SessionEventEnvelope;
use sqlx::SqlitePool;

use crate::error::DbError;
use crate::events::envelope_from_row;

/// セッション単位にまとめた未送信イベント。
///
/// `EventBatchPush` はセッション単位に分割して送信し、ACK もセッション単位で
/// 返却するため、抽出結果もセッションごとのバッチとする。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionOutboxBatch {
    /// 対象セッションID
    pub session_id: String,
    /// 未送信イベント (`node_seq` 昇順)
    pub events: Vec<SessionEventEnvelope>,
}

impl SessionOutboxBatch {
    /// バッチ内の最大 `node_seq` (ACK 待ち水位の検証用)。
    pub fn last_node_seq(&self) -> Option<u64> {
        self.events.last().map(|event| event.node_seq)
    }
}

/// 未送信イベントをセッション単位にまとめて抽出する。
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

/// `ResyncRequest` 用: 指定セッションの `from_node_seq` 以降 (`>=`) のイベントを
/// 水位に関係なく抽出する (`from_node_seq = 1` は `SessionCreated` を含む全量再送)。
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

/// `EventBatchAck` 受信時の水位更新。
///
/// 水位は単調増加 (`MAX`) とし、遅延して届いた古い ACK で巻き戻らないようにする。
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

/// 未同期 Outbox イベント件数 (UI の同期状態バッジ用)。
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

/// 最後に ACK されたイベントの `created_at` (最終同期時刻)。
///
/// 1件も同期していない場合は `None`。
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
