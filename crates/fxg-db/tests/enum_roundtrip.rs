//! INSERT -> SELECT roundtrips for the domain enums stored in TEXT columns.
//!
//! The wire / DB string representation is unchanged, so these tests guard against
//! the Rust enum vocabulary and the TEXT values drifting apart.

use std::collections::HashSet;

use fxg_db::registration::ProvisionalSessionRecord;
use fxg_db::{AuditLogRecord, Db, DbRole, NodeRecord};
use fxg_protocol::common::{
    AuditAction, ElicitationMode, ElicitationRequestEntry, ElicitationRequestStatus, EventType,
    NodeLifecycleStatus, NodeSummary, PermissionRequestEntry, PermissionRequestStatus, ResolvedBy,
    SessionStatus, SessionSummary,
};
use fxg_protocol::util::{now_ms, uuid_v7};
use sqlx::SqlitePool;

const NODE_ID: &str = "enum-test-node";
const PROJECT_ID: &str = "github.com/nazo6/flexagent";
const LOCAL_PATH: &str = "/home/nazo/src/flexagent";

const SESSION_STATUSES: &[SessionStatus] = &[
    SessionStatus::Provisioning,
    SessionStatus::Bootstrapping,
    SessionStatus::Idle,
    SessionStatus::Running,
    SessionStatus::WaitingPermission,
    SessionStatus::WaitingInput,
    SessionStatus::Stopped,
    SessionStatus::Error,
];

const NODE_LIFECYCLE_STATUSES: &[NodeLifecycleStatus] = &[
    NodeLifecycleStatus::Provisioning,
    NodeLifecycleStatus::Bootstrapping,
    NodeLifecycleStatus::Ready,
    NodeLifecycleStatus::Draining,
    NodeLifecycleStatus::Terminated,
    NodeLifecycleStatus::Error,
];

const PERMISSION_STATUSES: &[PermissionRequestStatus] = &[
    PermissionRequestStatus::Pending,
    PermissionRequestStatus::Approved,
    PermissionRequestStatus::Rejected,
    PermissionRequestStatus::Cancelled,
];

const ELICITATION_STATUSES: &[ElicitationRequestStatus] = &[
    ElicitationRequestStatus::Pending,
    ElicitationRequestStatus::Accepted,
    ElicitationRequestStatus::Declined,
    ElicitationRequestStatus::Cancelled,
];

fn resolved_by_values() -> Vec<ResolvedBy> {
    vec![
        ResolvedBy::Cli,
        ResolvedBy::Web,
        ResolvedBy::AndroidPush,
        ResolvedBy::Unknown("future_client".to_owned()),
    ]
}

fn elicitation_modes() -> Vec<ElicitationMode> {
    vec![
        ElicitationMode::Form,
        ElicitationMode::Url,
        ElicitationMode::Unknown("future_mode".to_owned()),
    ]
}

fn audit_actions() -> Vec<AuditAction> {
    vec![
        AuditAction::SessionStart,
        AuditAction::SessionRevert,
        AuditAction::SessionResume,
        AuditAction::SessionArchive,
        AuditAction::SessionDelete,
        AuditAction::PermissionResolved,
        AuditAction::ElicitationResolved,
        AuditAction::PtySpawn,
        AuditAction::KillSwitch,
        AuditAction::WorktreeManage,
        AuditAction::WorktreePrune,
        AuditAction::ProjectLink,
        AuditAction::AgentManage,
        AuditAction::NodeTokenManage,
        AuditAction::AuthTokenRotate,
        AuditAction::Unknown("future_action".to_owned()),
    ]
}

fn event_types() -> Vec<EventType> {
    vec![
        EventType::SessionCreated,
        EventType::SessionTitleChanged,
        EventType::SessionAgentBound,
        EventType::UserMessage,
        EventType::AgentMessage,
        EventType::AgentThought,
        EventType::ToolCall,
        EventType::Plan,
        EventType::PermissionRequest,
        EventType::PermissionResolved,
        EventType::ElicitationRequest,
        EventType::ElicitationResolved,
        EventType::SessionReverted,
        EventType::SessionArchived,
        EventType::SessionDeleted,
        EventType::TerminalOutput,
        EventType::TerminalInput,
        EventType::CapabilitiesUpdated,
        EventType::BootstrapLog,
        EventType::UsageUpdated,
        EventType::TurnEnded,
        EventType::CompactionUpdated,
        EventType::StatusChanged,
        EventType::Unknown("future_event".to_owned()),
    ]
}

/// Opens an in-memory node DB and creates one provisional session.
async fn fixture() -> (Db, String) {
    let db = Db::open_in_memory(DbRole::Node)
        .await
        .expect("open in-memory db");
    db.upsert_node(&NodeRecord::new(
        NODE_ID,
        "Enum Test Node",
        "linux",
        "x86_64",
        "0.1.0",
    ))
    .await
    .expect("upsert node");
    let session_id = uuid_v7();
    db.upsert_provisional_session(&ProvisionalSessionRecord {
        session_id: session_id.clone(),
        project_id: PROJECT_ID.to_owned(),
        project_name: "flexagent".to_owned(),
        node_id: NODE_ID.to_owned(),
        local_path: LOCAL_PATH.to_owned(),
        git_branch: Some("main".to_owned()),
        agent_id: "opencode2".to_owned(),
        title: "Enum roundtrip".to_owned(),
    })
    .await
    .expect("upsert provisional session");
    (db, session_id)
}

async fn session(db: &Db, session_id: &str) -> SessionSummary {
    db.get_session(session_id)
        .await
        .expect("get session")
        .expect("session exists")
}

#[tokio::test]
async fn session_status_roundtrips_every_variant() {
    let (db, session_id) = fixture().await;
    for status in SESSION_STATUSES {
        db.set_provisional_session_status(&session_id, *status)
            .await
            .expect("set status");
        assert_eq!(session(&db, &session_id).await.status, *status);
    }
}

#[tokio::test]
async fn node_lifecycle_status_roundtrips_every_variant() {
    let (db, _session_id) = fixture().await;
    for status in NODE_LIFECYCLE_STATUSES {
        let mut record = NodeRecord::new(NODE_ID, "Enum Test Node", "linux", "x86_64", "0.1.0");
        record.lifecycle_status = *status;
        db.upsert_node(&record).await.expect("upsert node");
        let node: NodeSummary = db
            .list_nodes()
            .await
            .expect("list nodes")
            .into_iter()
            .find(|node| node.node_id.as_str() == NODE_ID)
            .expect("node exists");
        assert_eq!(node.lifecycle_status, *status);
    }
}

/// Inserts a permission request row directly (all statuses are reachable only this way).
async fn insert_permission(
    pool: &SqlitePool,
    session_id: &str,
    status: PermissionRequestStatus,
    resolved_by: Option<&ResolvedBy>,
) -> String {
    let request_id = uuid_v7();
    sqlx::query(
        r#"
        INSERT INTO permission_requests (
            request_id, session_id, node_id, tool_name, summary, details_json,
            status, resolved_by, created_at, resolved_at
        )
        VALUES (?, ?, ?, ?, ?, '{}', ?, ?, ?, NULL)
        "#,
    )
    .bind(&request_id)
    .bind(session_id)
    .bind(NODE_ID)
    .bind("terminal/create")
    .bind("Run command: cargo test")
    .bind(status.as_str())
    .bind(resolved_by.map(ResolvedBy::as_str))
    .bind(now_ms())
    .execute(pool)
    .await
    .expect("insert permission request");
    request_id
}

async fn permission(db: &Db, request_id: &str) -> PermissionRequestEntry {
    db.find_permission_request(request_id)
        .await
        .expect("find permission")
        .expect("permission exists")
}

#[tokio::test]
async fn permission_status_and_resolved_by_roundtrip_all_variants() {
    let (db, session_id) = fixture().await;
    for status in PERMISSION_STATUSES {
        let request_id = insert_permission(db.pool(), &session_id, *status, None).await;
        assert_eq!(permission(&db, &request_id).await.status, *status);
    }
    for resolved_by in resolved_by_values() {
        let request_id = insert_permission(
            db.pool(),
            &session_id,
            PermissionRequestStatus::Approved,
            Some(&resolved_by),
        )
        .await;
        assert_eq!(
            permission(&db, &request_id).await.resolved_by,
            Some(resolved_by)
        );
    }
    // The pending inbox filter still finds the pending row.
    let pending: Vec<_> = db
        .pending_permissions()
        .await
        .expect("pending permissions")
        .into_iter()
        .filter(|entry| entry.session_id.as_str() == session_id)
        .collect();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, PermissionRequestStatus::Pending);
}

/// Inserts an elicitation row directly (all statuses are reachable only this way).
async fn insert_elicitation(
    pool: &SqlitePool,
    session_id: &str,
    mode: &ElicitationMode,
    status: ElicitationRequestStatus,
    resolved_by: Option<&ResolvedBy>,
) -> String {
    let elicitation_id = uuid_v7();
    sqlx::query(
        r#"
        INSERT INTO elicitation_requests (
            elicitation_id, session_id, node_id, message, mode, schema_json,
            tool_call_id, status, content_json, resolved_by, created_at, resolved_at
        )
        VALUES (?, ?, ?, ?, ?, ?, NULL, ?, 'null', ?, ?, NULL)
        "#,
    )
    .bind(&elicitation_id)
    .bind(session_id)
    .bind(NODE_ID)
    .bind("Which strategy should we use?")
    .bind(mode.as_str())
    .bind(r#"{"type":"object"}"#)
    .bind(status.as_str())
    .bind(resolved_by.map(ResolvedBy::as_str))
    .bind(now_ms())
    .execute(pool)
    .await
    .expect("insert elicitation request");
    elicitation_id
}

async fn elicitation(db: &Db, elicitation_id: &str) -> ElicitationRequestEntry {
    db.find_elicitation_request(elicitation_id)
        .await
        .expect("find elicitation")
        .expect("elicitation exists")
}

#[tokio::test]
async fn elicitation_mode_status_and_resolved_by_roundtrip_all_variants() {
    let (db, session_id) = fixture().await;
    for status in ELICITATION_STATUSES {
        let elicitation_id = insert_elicitation(
            db.pool(),
            &session_id,
            &ElicitationMode::Form,
            *status,
            None,
        )
        .await;
        assert_eq!(elicitation(&db, &elicitation_id).await.status, *status);
    }
    for mode in elicitation_modes() {
        let elicitation_id = insert_elicitation(
            db.pool(),
            &session_id,
            &mode,
            ElicitationRequestStatus::Pending,
            None,
        )
        .await;
        assert_eq!(elicitation(&db, &elicitation_id).await.mode, mode);
    }
    for resolved_by in resolved_by_values() {
        let elicitation_id = insert_elicitation(
            db.pool(),
            &session_id,
            &ElicitationMode::Form,
            ElicitationRequestStatus::Accepted,
            Some(&resolved_by),
        )
        .await;
        assert_eq!(
            elicitation(&db, &elicitation_id).await.resolved_by,
            Some(resolved_by)
        );
    }
    let pending: Vec<_> = db
        .pending_elicitations()
        .await
        .expect("pending elicitations")
        .into_iter()
        .filter(|entry| entry.session_id.as_str() == session_id)
        .collect();
    assert_eq!(pending.len(), 4);
    assert!(
        pending
            .iter()
            .all(|entry| entry.status == ElicitationRequestStatus::Pending)
    );
}

#[tokio::test]
async fn audit_action_roundtrips_all_variants() {
    let (db, _session_id) = fixture().await;
    let expected = audit_actions();
    for action in &expected {
        db.append_audit_log(&AuditLogRecord::new(action.clone(), "127.0.0.1", "test"))
            .await
            .expect("append audit log");
    }
    let recorded: HashSet<String> = db
        .audit_logs(1000)
        .await
        .expect("audit logs")
        .into_iter()
        .map(|entry| entry.action.as_str().to_owned())
        .collect();
    for action in &expected {
        assert!(
            recorded.contains(action.as_str()),
            "audit action {} was not roundtripped",
            action.as_str()
        );
    }
}

#[tokio::test]
async fn event_type_roundtrips_all_variants() {
    let (db, session_id) = fixture().await;
    let expected = event_types();
    for (index, event_type) in expected.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO session_events (
                event_id, session_id, node_seq, event_type, payload_json, searchable_text, created_at
            )
            VALUES (?, ?, ?, ?, '{}', 'ab', ?)
            "#,
        )
        .bind(uuid_v7())
        .bind(&session_id)
        .bind(i64::try_from(index + 1).expect("small index"))
        .bind(event_type.as_str())
        .bind(now_ms())
        .execute(db.pool())
        .await
        .expect("insert session event");
    }
    // A 2-character query hits the LIKE fallback, which returns every row.
    let found: HashSet<String> = db
        .search("ab", 500)
        .await
        .expect("search")
        .into_iter()
        .map(|hit| hit.event_type.as_str().to_owned())
        .collect();
    for event_type in &expected {
        assert!(
            found.contains(event_type.as_str()),
            "event type {} was not roundtripped",
            event_type.as_str()
        );
    }
}

#[tokio::test]
async fn unknown_session_status_is_not_silently_coerced() {
    let (db, session_id) = fixture().await;
    // Simulate a DB written by a newer peer with a status this build does not know.
    sqlx::query("UPDATE sessions SET status = 'future_status' WHERE session_id = ?")
        .bind(&session_id)
        .execute(db.pool())
        .await
        .expect("write unknown status");
    assert!(
        db.get_session(&session_id).await.is_err(),
        "a closed enum must reject unknown DB values instead of coercing them"
    );
}

#[tokio::test]
async fn command_id_sqlx_roundtrip() {
    let (db, _session_id) = fixture().await;
    sqlx::query("CREATE TEMP TABLE id_probe (id TEXT NOT NULL)")
        .execute(db.pool())
        .await
        .expect("create probe table");
    let id = fxg_protocol::CommandId::from("cmd-probe-1");
    sqlx::query("INSERT INTO id_probe (id) VALUES (?)")
        .bind(&id)
        .execute(db.pool())
        .await
        .expect("insert id");
    let decoded: fxg_protocol::CommandId = sqlx::query_scalar("SELECT id FROM id_probe")
        .fetch_one(db.pool())
        .await
        .expect("select id");
    assert_eq!(decoded, id);
}
