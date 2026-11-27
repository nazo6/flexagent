//! Composer draft persistence (`drafts`).

use fxg_db::{Db, DbError, Role};
use fxg_protocol::client_api::{CreateDraftRequest, DraftListQuery, UpdateDraftRequest};
use fxg_protocol::common::AttachmentMeta;
use fxg_protocol::util::now_ms;

async fn db() -> Db {
    Db::open_in_memory(Role::Node).await.expect("db")
}

fn create(body: &str, session_id: Option<&str>) -> CreateDraftRequest {
    CreateDraftRequest {
        title: String::new(),
        body: body.to_owned(),
        session_id: session_id.map(Into::into),
        project_id: None,
        node_id: None,
        attachments: Vec::new(),
        origin_device_id: "device-test".to_owned(),
    }
}

fn update(revision: u64, body: &str) -> UpdateDraftRequest {
    UpdateDraftRequest {
        expected_revision: revision,
        title: String::new(),
        body: body.to_owned(),
        session_id: None,
        project_id: None,
        node_id: None,
        attachments: Vec::new(),
        origin_device_id: "device-test".to_owned(),
    }
}

#[tokio::test]
async fn drafts_roundtrip_and_filter_by_session() {
    let db = db().await;
    let session = db
        .create_draft(&create("session body", Some("session-a")))
        .await
        .unwrap();
    let loose = db.create_draft(&create("loose body", None)).await.unwrap();

    let all = db.list_drafts(&DraftListQuery::default()).await.unwrap();
    assert_eq!(all.len(), 2);

    let by_session = db
        .list_drafts(&DraftListQuery {
            session_id: Some("session-a".into()),
            ..DraftListQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(by_session.len(), 1);
    assert_eq!(by_session[0].draft_id, session.draft_id);

    let sessionless = db
        .list_drafts(&DraftListQuery {
            sessionless: Some(true),
            ..DraftListQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(sessionless.len(), 1);
    assert_eq!(sessionless[0].draft_id, loose.draft_id);
}

#[tokio::test]
async fn draft_updates_use_optimistic_revision() {
    let db = db().await;
    let created = db.create_draft(&create("first", None)).await.unwrap();
    assert_eq!(created.revision, 1);

    let updated = db
        .update_draft(created.draft_id.as_str(), &update(1, "second"))
        .await
        .unwrap()
        .expect("draft");
    assert_eq!(updated.body, "second");
    assert_eq!(updated.revision, 2);

    let err = db
        .update_draft(created.draft_id.as_str(), &update(1, "stale"))
        .await
        .expect_err("stale revision must fail");
    assert!(matches!(err, DbError::RevisionMismatch));

    let stored = db
        .get_draft(created.draft_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.body, "second");
    assert_eq!(stored.revision, 2);
}

#[tokio::test]
async fn draft_delete_is_a_tombstone_and_idempotent() {
    let db = db().await;
    let created = db.create_draft(&create("body", None)).await.unwrap();

    let revision = db.delete_draft(created.draft_id.as_str()).await.unwrap();
    assert_eq!(revision, Some(2));
    assert!(
        db.delete_draft(created.draft_id.as_str())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db.get_draft(created.draft_id.as_str())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db.list_drafts(&DraftListQuery::default())
            .await
            .unwrap()
            .is_empty()
    );

    // Updating a deleted draft reports "not found" instead of a revision conflict.
    assert!(
        db.update_draft(created.draft_id.as_str(), &update(2, "after delete"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn draft_attachments_roundtrip_as_metadata() {
    let db = db().await;
    let mut request = create("", None);
    request.attachments = vec![AttachmentMeta {
        file_name: "image.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size_bytes: 4,
        local_path: None,
        data_b64: None,
    }];
    let created = db.create_draft(&request).await.unwrap();
    assert_eq!(created.attachments.len(), 1);
    assert_eq!(created.attachments[0].file_name, "image.png");

    // A newer draft sorts first (the list is `updated_at DESC`).
    let second = db.create_draft(&create("newest", None)).await.unwrap();
    assert!(second.updated_at >= created.updated_at);
    assert!(now_ms() >= second.created_at);
}
