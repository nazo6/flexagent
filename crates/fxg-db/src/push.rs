//! Web Push 購読 (`push_subscriptions`) の管理。
//!
//! 中央サーバーのみが使用する (Android / Desktop PWA の Web Push 通知)。
//! 購読は端末ごとに `endpoint` で一意化し、再登録時は鍵を更新する
//! (設計: `docs/03-protocol-and-api.md` §3.1、`docs/05-cli-and-pwa-ui.md` §3.3)。

use fxg_protocol::util::now_ms;
use sqlx::SqlitePool;

use crate::error::DbError;

/// Web Push 購読レコード。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSubscriptionRecord {
    /// Push サービスエンドポイント URL
    pub endpoint: String,
    /// 購読の `p256dh` 公開鍵 (Base64)
    pub p256dh: String,
    /// 購読の `auth` シークレット (Base64)
    pub auth: String,
    /// 端末名 (例: `Pixel 9 Chrome PWA`)
    pub device_name: Option<String>,
}

/// 購読を登録・更新する (同一 `endpoint` は鍵を上書きする)。
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

/// 全購読を取得する (通知送信用)。
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

/// 失効した購読 (`404` / `410` 応答) を削除する。
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

/// sqlx の型安全マクロ用行構造体。
struct PushSubscriptionRow {
    endpoint: String,
    p256dh: String,
    auth: String,
    device_name: Option<String>,
}
