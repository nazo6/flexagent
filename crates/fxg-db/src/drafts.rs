//! Composer draft persistence.
//!
//! Design: `docs/proposals/2026-11-27-composer-drafts-and-sync.md`.
//!
//! Drafts are a small, mutable, client-owned aggregate. They deliberately do
//! not go through `session_events` (append-only, `node_seq`-ordered execution
//! history); updates use a revision-based optimistic lock so a stale device
//! cannot silently overwrite a newer body.

use fxg_protocol::client_api::{CreateDraftRequest, Draft, DraftListQuery, UpdateDraftRequest};
use fxg_protocol::common::AttachmentMeta;
use fxg_protocol::util::{now_ms, uuid_v7};
use fxg_protocol::{DraftId, NodeId, ProjectId, SessionId};
use sqlx::SqlitePool;

use crate::error::DbError;

/// Maximum number of drafts returned by one list request.
const MAX_DRAFT_PAGE: u32 = 1000;
/// Default number of drafts returned by one list request.
const DEFAULT_DRAFT_PAGE: u32 = 200;

fn revision_u64(revision: i64) -> Result<u64, DbError> {
    u64::try_from(revision)
        .map_err(|err| DbError::InvalidPayload(format!("negative draft revision: {err}")))
}

fn attachments_from_json(value: &str) -> Result<Vec<AttachmentMeta>, DbError> {
    Ok(serde_json::from_str(value)?)
}

fn attachments_to_json(attachments: &[AttachmentMeta]) -> Result<String, DbError> {
    Ok(serde_json::to_string(attachments)?)
}

/// Lists active drafts, newest first.
pub async fn list_drafts(pool: &SqlitePool, query: &DraftListQuery) -> Result<Vec<Draft>, DbError> {
    let session_id = query.session_id.as_ref().map(SessionId::as_str);
    let sessionless = query.sessionless.map(|value| if value { 1 } else { 0 });
    let project_id = query.project_id.as_ref().map(ProjectId::as_str);
    let limit = i64::from(
        query
            .limit
            .unwrap_or(DEFAULT_DRAFT_PAGE)
            .clamp(1, MAX_DRAFT_PAGE),
    );

    let rows = sqlx::query!(
        r#"
        SELECT draft_id AS "draft_id!", title, body, session_id, project_id, node_id,
               attachments_json, revision, origin_device_id, created_at, updated_at
          FROM drafts
         WHERE deleted_at IS NULL
           AND (?1 IS NULL OR session_id = ?1)
           AND (?2 IS NULL OR (?2 = 1 AND session_id IS NULL))
           AND (?3 IS NULL OR project_id = ?3)
         ORDER BY updated_at DESC, draft_id ASC
         LIMIT ?4
        "#,
        session_id,
        sessionless,
        project_id,
        limit,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(Draft {
                draft_id: DraftId::from(row.draft_id),
                title: row.title,
                body: row.body,
                session_id: row.session_id.map(SessionId::from),
                project_id: row.project_id.map(ProjectId::from),
                node_id: row.node_id.map(NodeId::from),
                attachments: attachments_from_json(&row.attachments_json)?,
                revision: revision_u64(row.revision)?,
                origin_device_id: row.origin_device_id,
                created_at: row.created_at,
                updated_at: row.updated_at,
            })
        })
        .collect()
}

/// Loads one active draft.
pub async fn get_draft(pool: &SqlitePool, draft_id: &str) -> Result<Option<Draft>, DbError> {
    let row = sqlx::query!(
        r#"
        SELECT draft_id AS "draft_id!", title, body, session_id, project_id, node_id,
               attachments_json, revision, origin_device_id, created_at, updated_at
          FROM drafts
         WHERE draft_id = ? AND deleted_at IS NULL
        "#,
        draft_id,
    )
    .fetch_optional(pool)
    .await?;

    row.map(|row| {
        Ok(Draft {
            draft_id: DraftId::from(row.draft_id),
            title: row.title,
            body: row.body,
            session_id: row.session_id.map(SessionId::from),
            project_id: row.project_id.map(ProjectId::from),
            node_id: row.node_id.map(NodeId::from),
            attachments: attachments_from_json(&row.attachments_json)?,
            revision: revision_u64(row.revision)?,
            origin_device_id: row.origin_device_id,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    })
    .transpose()
}

/// Creates a draft and returns the stored row.
pub async fn create_draft(
    pool: &SqlitePool,
    request: &CreateDraftRequest,
) -> Result<Draft, DbError> {
    let draft_id = uuid_v7();
    let now = now_ms();
    let attachments_json = attachments_to_json(&request.attachments)?;
    sqlx::query!(
        r#"
        INSERT INTO drafts
            (draft_id, title, body, session_id, project_id, node_id, attachments_json,
             revision, origin_device_id, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)
        "#,
        draft_id,
        request.title,
        request.body,
        request.session_id.as_ref().map(SessionId::as_str),
        request.project_id.as_ref().map(ProjectId::as_str),
        request.node_id.as_ref().map(NodeId::as_str),
        attachments_json,
        request.origin_device_id,
        now,
        now,
    )
    .execute(pool)
    .await?;

    get_draft(pool, &draft_id)
        .await?
        .ok_or_else(|| DbError::InvalidPayload("created draft is missing".to_owned()))
}

/// Replaces a draft when `expected_revision` still matches.
///
/// Returns `Ok(None)` when the draft does not exist (or was deleted).
pub async fn update_draft(
    pool: &SqlitePool,
    draft_id: &str,
    request: &UpdateDraftRequest,
) -> Result<Option<Draft>, DbError> {
    let expected = i64::try_from(request.expected_revision).unwrap_or(i64::MAX);
    let current = sqlx::query_scalar!(
        r#"SELECT revision FROM drafts WHERE draft_id = ? AND deleted_at IS NULL"#,
        draft_id,
    )
    .fetch_optional(pool)
    .await?;
    let Some(current) = current else {
        return Ok(None);
    };
    if current != expected {
        return Err(DbError::RevisionMismatch);
    }

    let attachments_json = attachments_to_json(&request.attachments)?;
    let now = now_ms();
    let result = sqlx::query!(
        r#"
        UPDATE drafts
           SET title = ?, body = ?, session_id = ?, project_id = ?, node_id = ?,
               attachments_json = ?, revision = revision + 1, origin_device_id = ?,
               updated_at = ?
         WHERE draft_id = ? AND revision = ? AND deleted_at IS NULL
        "#,
        request.title,
        request.body,
        request.session_id.as_ref().map(SessionId::as_str),
        request.project_id.as_ref().map(ProjectId::as_str),
        request.node_id.as_ref().map(NodeId::as_str),
        attachments_json,
        request.origin_device_id,
        now,
        draft_id,
        expected,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        // Another writer won the race between the revision check and the update.
        return Err(DbError::RevisionMismatch);
    }

    get_draft(pool, draft_id).await
}

/// Soft-deletes a draft (idempotent).
///
/// Returns the new revision when an active row was tombstoned.
pub async fn delete_draft(pool: &SqlitePool, draft_id: &str) -> Result<Option<u64>, DbError> {
    let revision = sqlx::query_scalar!(
        r#"
        UPDATE drafts
           SET deleted_at = ?, revision = revision + 1
         WHERE draft_id = ? AND deleted_at IS NULL
        RETURNING revision
        "#,
        now_ms(),
        draft_id,
    )
    .fetch_optional(pool)
    .await?;
    revision.map(revision_u64).transpose()
}
