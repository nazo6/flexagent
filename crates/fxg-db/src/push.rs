//! Web Push subscription management (`push_subscriptions`).
//!
//! Used only by the hub (Web Push notifications for Android / Desktop PWA).
//! Subscriptions are keyed per device by `endpoint`, and keys are updated on re-registration
//! (design: `docs/03-protocol-and-api.md` §3.1, `docs/07-web-ui-and-pwa.md` §3.3).

use fxg_protocol::util::now_ms;
use sqlx::SqlitePool;

use crate::error::DbError;

/// Web Push subscription record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSubscriptionRecord {
    /// Push service endpoint URL
    pub endpoint: String,
    /// Subscription's `p256dh` public key (Base64)
    pub p256dh: String,
    /// Subscription's `auth` secret (Base64)
    pub auth: String,
    /// Device name (e.g. `Pixel 9 Chrome PWA`)
    pub device_name: Option<String>,
}

/// Registers or updates a subscription (same `endpoint` overwrites the keys).
pub async fn upsert_push_subscription(
    pool: &SqlitePool,
    record: &PushSubscriptionRecord,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO push_subscriptions (endpoint, p256dh, auth, device_name, created_at)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(endpoint) DO UPDATE SET
            p256dh = excluded.p256dh,
            auth = excluded.auth,
            device_name = excluded.device_name
        "#,
        record.endpoint,
        record.p256dh,
        record.auth,
        record.device_name.as_deref(),
        now_ms(),
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Lists all subscriptions (for sending notifications).
pub async fn list_push_subscriptions(
    pool: &SqlitePool,
) -> Result<Vec<PushSubscriptionRecord>, DbError> {
    let rows = sqlx::query_as!(
        PushSubscriptionRow,
        r#"
        SELECT endpoint, p256dh, auth, device_name
          FROM push_subscriptions
         ORDER BY created_at ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| PushSubscriptionRecord {
            endpoint: row.endpoint,
            p256dh: row.p256dh,
            auth: row.auth,
            device_name: row.device_name,
        })
        .collect())
}

/// Deletes an expired subscription (`404` / `410` responses).
pub async fn delete_push_subscription(pool: &SqlitePool, endpoint: &str) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        DELETE FROM push_subscriptions
         WHERE endpoint = ?
        "#,
        endpoint,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Row struct for the type-safe sqlx macros.
struct PushSubscriptionRow {
    endpoint: String,
    p256dh: String,
    auth: String,
    device_name: Option<String>,
}
