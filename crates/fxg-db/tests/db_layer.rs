//! Integration tests for `fxg-db`.
//!
//! Verification of the Phase 1 completion criteria:
//! - Migration application (single schema + FTS5 + triggers)
//! - Idempotency of event application (`INSERT OR IGNORE` + same-transaction projections)
//! - Projection rebuild matches event application results
//! - Watermark-based outbox extraction and ACK
//! - FTS5 full-text search (via triggers, `searchable_text` exclusion rules, LIKE fallback)
//! - Local Node (`node.db`) and Hub (`hub.db`) return identical response types

use fxg_db::audit::actions;
use fxg_db::{
    AuditLogRecord, Db, DbError, NodeRecord, ProjectBindingRecord, ProjectRecord, Role,
    SessionFilter,
};
use fxg_protocol::common::{
    ClientSource, CommandInfo, ElicitationAction, ElicitationMode, ElicitationRequestStatus,
    EventType, ForkScope, ModeInfo, PermissionOption, PermissionOptionKind,
    PermissionRequestStatus, PlanEntry, PlanEntryStatus, ResolvedBy, SessionCursor, SessionStatus,
    UsageCost,
};
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use fxg_protocol::util::{now_ms, uuid_v7};
use fxg_protocol::{NodeId, ProjectId, SessionId};

const PROJECT_ID: &str = "github.com/nazo6/flexagent";
const NODE_ID: &str = "test-node";
const LOCAL_PATH: &str = "/home/nazo/src/flexagent";

/// Test environment (DB with node/project registered, plus an event sequencing helper).
struct Fixture {
    db: Db,
    session_id: String,
    seq: u64,
}

impl Fixture {
    async fn new(role: Role) -> Self {
        let session_id = uuid_v7();
        Self::with_session_id(role, session_id).await
    }

    async fn with_session_id(role: Role, session_id: String) -> Self {
        let db = Db::open_in_memory(role).await.expect("open in-memory db");
        db.upsert_node(
            &NodeRecord::new(NODE_ID, "Test Node", "linux", "aarch64", "0.1.0")
                .with_agents(vec!["opencode2".to_owned()]),
        )
        .await
        .expect("upsert node");
        db.upsert_project(&ProjectRecord {
            project_id: PROJECT_ID.into(),
            name: "flexagent".to_owned(),
            canonical_git_url: Some("git@github.com:nazo6/flexagent.git".to_owned()),
            identity: fxg_protocol::settings::ProjectIdentity::git_remote(),
        })
        .await
        .expect("upsert project");
        db.upsert_project_binding(&ProjectBindingRecord {
            project_id: PROJECT_ID.into(),
            node_id: NODE_ID.into(),
            local_path: LOCAL_PATH.to_owned(),
            is_worktree: false,
            path_exists: true,
            git_branch: Some("main".to_owned()),
        })
        .await
        .expect("upsert binding");
        Self {
            db,
            session_id,
            seq: 0,
        }
    }

    /// Builds a new event (auto-allocating node_seq).
    fn event(&mut self, payload: UnifiedEventPayload) -> SessionEventEnvelope {
        self.seq += 1;
        SessionEventEnvelope {
            event_id: uuid_v7().into(),
            session_id: self.session_id.clone().into(),
            node_seq: self.seq,
            created_at: now_ms(),
            payload,
        }
    }

    fn session_created(&mut self) -> SessionEventEnvelope {
        self.event(session_created_payload())
    }

    /// Feeds a typical event stream (SessionCreated → title → agent bound → status →
    /// capabilities → user message → permission request → permission resolution).
    async fn seed_typical_stream(&mut self) -> Vec<SessionEventEnvelope> {
        let events = vec![
            self.session_created(),
            self.event(UnifiedEventPayload::SessionTitleChanged {
                title: "Fix authentication error".to_owned(),
            }),
            self.event(UnifiedEventPayload::SessionAgentBound {
                agent_session_id: "agent-sess-1".to_owned(),
            }),
            self.event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            }),
            self.event(UnifiedEventPayload::CapabilitiesUpdated {
                current_mode: Some("code".to_owned()),
                available_modes: vec![ModeInfo {
                    mode_id: "plan".to_owned(),
                    name: "Plan".to_owned(),
                    description: None,
                }],
                available_commands: vec![CommandInfo {
                    name: "/review".to_owned(),
                    description: "Code review".to_owned(),
                    input_hint: None,
                }],
                config_options: vec![],
                fork_scope: Some(ForkScope::Last),
                native_features: Vec::new(),
            }),
            self.event(UnifiedEventPayload::UserMessage {
                text: "Fix the authentication error".to_owned(),
                attachments: vec![],
                client_source: ClientSource::Cli,
                agent_message_id: None,
            }),
            self.event(UnifiedEventPayload::PermissionRequest {
                request_id: "req-1".to_owned(),
                tool_name: "terminal/create".to_owned(),
                summary: "Run command: cargo test --workspace".to_owned(),
                options: vec![
                    PermissionOption {
                        option_id: "allow_once".to_owned(),
                        name: "Allow once".to_owned(),
                        kind: PermissionOptionKind::AllowOnce,
                    },
                    PermissionOption {
                        option_id: "reject_once".to_owned(),
                        name: "Reject".to_owned(),
                        kind: PermissionOptionKind::RejectOnce,
                    },
                ],
                details: serde_json::json!({ "command": "cargo test --workspace" }).into(),
                subagent_id: None,
            }),
            self.event(UnifiedEventPayload::PermissionResolved {
                request_id: "req-1".to_owned(),
                selected_option_id: "allow_once".to_owned(),
                resolved_by: ResolvedBy::Cli,
            }),
        ];
        self.db.append_events(&events).await.expect("append events");
        events
    }
}

fn session_created_payload() -> UnifiedEventPayload {
    UnifiedEventPayload::SessionCreated {
        node_id: NODE_ID.into(),
        project_id: PROJECT_ID.into(),
        project_name: "flexagent".to_owned(),
        local_path: LOCAL_PATH.to_owned(),
        git_branch: Some("main".to_owned()),
        is_worktree: false,
        agent_id: "opencode2".to_owned(),
        parent_session_id: None,
        title: "New Session".to_owned(),
    }
}

async fn count_events(db: &Db, session_id: &str) -> i64 {
    let batch = db
        .session_events_after(session_id, 0, 1000)
        .await
        .expect("events");
    batch.events.len() as i64
}

/// Reads and parses `sessions.available_modes_json`.
async fn available_modes(db: &Db, session_id: &str) -> Vec<ModeInfo> {
    let json: String =
        sqlx::query_scalar(r#"SELECT available_modes_json FROM sessions WHERE session_id = ?"#)
            .bind(session_id)
            .fetch_one(db.pool())
            .await
            .expect("query available_modes_json");
    serde_json::from_str(&json).expect("parse available_modes_json")
}

#[tokio::test]
async fn migrations_create_single_schema_with_fts_and_triggers() {
    let db = Db::open_in_memory(Role::Node).await.expect("migrate");

    let tables: Vec<String> = sqlx::query_scalar(
        r#"SELECT name FROM sqlite_master WHERE type IN ('table','virtual table') ORDER BY name"#,
    )
    .fetch_all(db.pool())
    .await
    .expect("query sqlite_master");

    for expected in [
        "nodes",
        "projects",
        "project_node_bindings",
        "sessions",
        "session_events",
        "session_events_fts",
        "permission_requests",
        "elicitation_requests",
        "push_subscriptions",
        "audit_logs",
    ] {
        assert!(
            tables.iter().any(|name| name == expected),
            "missing table: {expected} (have: {tables:?})"
        );
    }

    let triggers: Vec<String> = sqlx::query_scalar(
        r#"SELECT name FROM sqlite_master WHERE type = 'trigger' ORDER BY name"#,
    )
    .fetch_all(db.pool())
    .await
    .expect("query triggers");
    for expected in [
        "session_events_fts_ai",
        "session_events_fts_ad",
        "session_events_fts_au",
    ] {
        assert!(
            triggers.iter().any(|name| name == expected),
            "missing trigger: {expected}"
        );
    }
}

#[tokio::test]
async fn event_application_is_idempotent() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();

    let first = fixture
        .db
        .append_event(&created)
        .await
        .expect("first append");
    assert!(first.inserted);

    // Resend of the same event (same event_id)
    let second = fixture
        .db
        .append_event(&created)
        .await
        .expect("second append");
    assert!(!second.inserted, "duplicate event must be ignored");

    // Same (session_id, node_seq) but different event_id (resend with a new allocation)
    let mut duplicate_seq = created.clone();
    duplicate_seq.event_id = uuid_v7().into();
    let third = fixture
        .db
        .append_event(&duplicate_seq)
        .await
        .expect("third append");
    assert!(!third.inserted, "(session_id, node_seq) UNIQUE must hold");

    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 1);

    // Projections are not applied twice either (title and watermark unchanged)
    let summary = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get session")
        .expect("session exists");
    assert_eq!(summary.title, "New Session");
    assert_eq!(summary.last_node_seq, 1);
}

#[tokio::test]
async fn duplicate_session_created_does_not_revert_title() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let title_changed = fixture.event(UnifiedEventPayload::SessionTitleChanged {
        title: "Fix authentication error".to_owned(),
    });
    fixture
        .db
        .append_events(&[created.clone(), title_changed.clone()])
        .await
        .expect("initial append");

    // A resync / duplicated batch replays the same events.
    let replay = fixture
        .db
        .append_events(&[created, title_changed])
        .await
        .expect("replay");
    assert!(replay.iter().all(|outcome| !outcome.inserted));

    let summary = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get session")
        .expect("session exists");
    assert_eq!(
        summary.title, "Fix authentication error",
        "a replayed SessionCreated must not roll back the title"
    );
    assert_eq!(summary.last_node_seq, 2);
}

#[tokio::test]
async fn event_application_updates_session_projections() {
    let mut fixture = Fixture::new(Role::Node).await;
    fixture.seed_typical_stream().await;

    let summary = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get session")
        .expect("session exists");
    assert_eq!(summary.project_id.as_str(), PROJECT_ID);
    assert_eq!(summary.node_id.as_str(), NODE_ID);
    assert_eq!(summary.local_path, LOCAL_PATH);
    assert_eq!(summary.git_branch.as_deref(), Some("main"));
    assert!(!summary.is_worktree);
    assert_eq!(summary.agent_id, "opencode2");
    assert_eq!(summary.agent_session_id.as_deref(), Some("agent-sess-1"));
    assert_eq!(summary.title, "Fix authentication error");
    assert_eq!(summary.status, SessionStatus::Running);
    assert_eq!(summary.current_mode.as_deref(), Some("code"));
    assert_eq!(summary.last_node_seq, 8);

    // available_modes from CapabilitiesUpdated are also reflected in projection columns
    assert_eq!(
        available_modes(&fixture.db, &fixture.session_id).await,
        vec![ModeInfo {
            mode_id: "plan".to_owned(),
            name: "Plan".to_owned(),
            description: None,
        }]
    );

    // The permission is projected as resolved
    let request = fixture
        .db
        .find_permission_request(&fixture.session_id, "req-1")
        .await
        .expect("find request")
        .expect("request exists");
    assert_eq!(request.status, PermissionRequestStatus::Approved);
    assert_eq!(request.resolved_by, Some(ResolvedBy::Cli));
    assert!(request.resolved_at.is_some());
    assert_eq!(request.options.len(), 2);
    assert_eq!(request.summary, "Run command: cargo test --workspace");
    assert_eq!(
        request.details.as_value()["command"],
        "cargo test --workspace"
    );
    assert!(
        fixture
            .db
            .pending_permissions()
            .await
            .expect("pending")
            .is_empty()
    );
}

#[tokio::test]
async fn capabilities_partial_update_keeps_available_modes_projection() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let modes = fixture.event(UnifiedEventPayload::CapabilitiesUpdated {
        current_mode: Some("plan".to_owned()),
        available_modes: vec![ModeInfo {
            mode_id: "plan".to_owned(),
            name: "Plan".to_owned(),
            description: None,
        }],
        available_commands: vec![],
        config_options: vec![],
        fork_scope: None,
        native_features: Vec::new(),
    });
    // Partial updates without modes (commands only) do not overwrite available_modes
    let commands = fixture.event(UnifiedEventPayload::CapabilitiesUpdated {
        current_mode: None,
        available_modes: vec![],
        available_commands: vec![CommandInfo {
            name: "/review".to_owned(),
            description: "Code review".to_owned(),
            input_hint: None,
        }],
        config_options: vec![],
        fork_scope: None,
        native_features: Vec::new(),
    });
    fixture
        .db
        .append_events(&[created, modes, commands])
        .await
        .expect("append");

    let modes = available_modes(&fixture.db, &fixture.session_id).await;
    assert_eq!(modes.len(), 1);
    assert_eq!(modes[0].mode_id, "plan");

    // A full rebuild reproduces the same projections
    fixture.db.rebuild_projections().await.expect("rebuild");
    let rebuilt = available_modes(&fixture.db, &fixture.session_id).await;
    assert_eq!(rebuilt.len(), 1);
    assert_eq!(rebuilt[0].mode_id, "plan");
}

#[tokio::test]
async fn fork_scope_projection_tracks_latest_and_rebuilds() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let caps = fixture.event(UnifiedEventPayload::CapabilitiesUpdated {
        current_mode: None,
        available_modes: vec![],
        available_commands: vec![],
        config_options: vec![],
        fork_scope: Some(ForkScope::Last),
        native_features: Vec::new(),
    });
    fixture
        .db
        .append_events(&[created, caps])
        .await
        .expect("append");

    let session = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("query")
        .expect("session");
    assert_eq!(session.fork_scope, ForkScope::Last);

    fixture.db.rebuild_projections().await.expect("rebuild");
    let rebuilt = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("query")
        .expect("session");
    assert_eq!(rebuilt.fork_scope, ForkScope::Last);
}

#[tokio::test]
async fn usage_projection_tracks_latest_and_rebuilds() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let first = fixture.event(UnifiedEventPayload::UsageUpdated {
        used_tokens: 1_000,
        context_size: 100_000,
        cost: None,
        subagent_id: None,
    });
    // The second update fully overwrites the first (keeps the latest, not cumulative)
    let second = fixture.event(UnifiedEventPayload::UsageUpdated {
        used_tokens: 53_000,
        context_size: 200_000,
        cost: Some(UsageCost {
            amount: 0.045,
            currency: "USD".to_owned(),
        }),
        subagent_id: None,
    });
    fixture
        .db
        .append_events(&[created, first, second])
        .await
        .expect("append");

    let session = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get")
        .expect("exists");
    let usage = session.usage.as_ref().expect("usage projected");
    assert_eq!(usage.used_tokens, 53_000);
    assert_eq!(usage.context_size, 200_000);
    let cost = usage.cost.as_ref().expect("cost");
    assert_eq!(cost.currency, "USD");
    assert!((cost.amount - 0.045).abs() < f64::EPSILON);

    // A full rebuild reproduces the same projections
    fixture.db.rebuild_projections().await.expect("rebuild");
    let rebuilt = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get after rebuild")
        .expect("exists");
    assert_eq!(rebuilt.usage, session.usage);
}

#[tokio::test]
async fn permission_rejection_is_projected_from_option_kind() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let request = fixture.event(UnifiedEventPayload::PermissionRequest {
        request_id: "req-reject".to_owned(),
        tool_name: "fs/write_text_file".to_owned(),
        summary: "Write file: src/main.rs".to_owned(),
        options: vec![PermissionOption {
            option_id: "no".to_owned(),
            name: "Reject".to_owned(),
            kind: PermissionOptionKind::RejectOnce,
        }],
        details: serde_json::json!({}).into(),
        subagent_id: None,
    });
    fixture
        .db
        .append_events(&[created, request])
        .await
        .expect("append");

    let pending = fixture.db.pending_permissions().await.expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].request_id, "req-reject");

    let resolved = fixture.event(UnifiedEventPayload::PermissionResolved {
        request_id: "req-reject".to_owned(),
        selected_option_id: "no".to_owned(),
        resolved_by: ResolvedBy::AndroidPush,
    });
    fixture.db.append_event(&resolved).await.expect("append");

    let request = fixture
        .db
        .find_permission_request(&fixture.session_id, "req-reject")
        .await
        .expect("find")
        .expect("exists");
    assert_eq!(request.status, PermissionRequestStatus::Rejected);
    assert_eq!(request.resolved_by, Some(ResolvedBy::AndroidPush));
}

#[tokio::test]
async fn permission_ids_are_scoped_to_their_session() {
    let mut first = Fixture::new(Role::Hub).await;
    let first_id = first.session_id.clone();
    let second_id = uuid_v7();
    let mut second = Fixture {
        db: first.db.clone(),
        session_id: second_id.clone(),
        seq: 0,
    };

    let first_created = first.session_created();
    let second_created = second.session_created();
    let options = vec![PermissionOption {
        option_id: "allow_once".to_owned(),
        name: "Allow once".to_owned(),
        kind: PermissionOptionKind::AllowOnce,
    }];
    let first_request = first.event(UnifiedEventPayload::PermissionRequest {
        request_id: "same-id".to_owned(),
        tool_name: "terminal/create".to_owned(),
        summary: "first".to_owned(),
        options: options.clone(),
        details: serde_json::json!({}).into(),
        subagent_id: None,
    });
    let second_request = second.event(UnifiedEventPayload::PermissionRequest {
        request_id: "same-id".to_owned(),
        tool_name: "terminal/create".to_owned(),
        summary: "second".to_owned(),
        options,
        details: serde_json::json!({}).into(),
        subagent_id: None,
    });
    first
        .db
        .append_events(&[first_created, first_request, second_created, second_request])
        .await
        .expect("append");

    let pending = first.db.pending_permissions().await.expect("pending");
    assert_eq!(pending.len(), 2, "both sessions keep their own request");
    assert!(
        pending
            .iter()
            .any(|entry| entry.session_id.as_str() == first_id)
    );
    assert!(
        pending
            .iter()
            .any(|entry| entry.session_id.as_str() == second_id)
    );

    let resolved = first.event(UnifiedEventPayload::PermissionResolved {
        request_id: "same-id".to_owned(),
        selected_option_id: "allow_once".to_owned(),
        resolved_by: ResolvedBy::Web,
    });
    first.db.append_event(&resolved).await.expect("resolve");

    let first_entry = first
        .db
        .find_permission_request(&first_id, "same-id")
        .await
        .expect("find first")
        .expect("first exists");
    assert_eq!(first_entry.status, PermissionRequestStatus::Approved);
    let second_entry = first
        .db
        .find_permission_request(&second_id, "same-id")
        .await
        .expect("find second")
        .expect("second exists");
    assert_eq!(
        second_entry.status,
        PermissionRequestStatus::Pending,
        "resolving one session must not resolve the other"
    );
}

#[tokio::test]
async fn elicitation_projection_tracks_pending_and_resolved() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let request = fixture.event(UnifiedEventPayload::ElicitationRequest {
        elicitation_id: "elic-1".to_owned(),
        message: "Which strategy should we use?".to_owned(),
        mode: ElicitationMode::Form,
        requested_schema: serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "strategy": { "type": "string", "enum": ["a", "b"] }
            },
            "required": ["strategy"]
        }))
        .expect("schema"),
        tool_call_id: Some("tool-1".to_owned()),
        subagent_id: None,
    });
    fixture
        .db
        .append_events(&[created, request])
        .await
        .expect("append");

    let pending = fixture.db.pending_elicitations().await.expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].elicitation_id, "elic-1");
    assert_eq!(pending[0].status, ElicitationRequestStatus::Pending);
    assert_eq!(pending[0].tool_call_id.as_deref(), Some("tool-1"));
    assert_eq!(pending[0].requested_schema.schema_type.as_str(), "object");

    let resolved = fixture.event(UnifiedEventPayload::ElicitationResolved {
        elicitation_id: "elic-1".to_owned(),
        action: ElicitationAction::Accept,
        content: Some(
            serde_json::from_value(serde_json::json!({ "strategy": "a" })).expect("content"),
        ),
        resolved_by: ResolvedBy::Web,
    });
    fixture.db.append_event(&resolved).await.expect("append");

    let entry = fixture
        .db
        .find_elicitation_request(&fixture.session_id, "elic-1")
        .await
        .expect("find")
        .expect("exists");
    assert_eq!(entry.status, ElicitationRequestStatus::Accepted);
    assert_eq!(
        entry.content.as_ref().expect("content").get("strategy"),
        Some(&fxg_protocol::common::ElicitationValue::String(
            "a".to_owned()
        ))
    );
    assert_eq!(entry.resolved_by, Some(ResolvedBy::Web));
    assert!(
        fixture
            .db
            .pending_elicitations()
            .await
            .expect("pending")
            .is_empty()
    );

    // A full rebuild reproduces the same projections
    fixture.db.rebuild_projections().await.expect("rebuild");
    let rebuilt = fixture
        .db
        .find_elicitation_request(&fixture.session_id, "elic-1")
        .await
        .expect("find after rebuild")
        .expect("exists");
    assert_eq!(rebuilt.status, ElicitationRequestStatus::Accepted);
    assert_eq!(
        rebuilt.content.as_ref().expect("content").get("strategy"),
        Some(&fxg_protocol::common::ElicitationValue::String(
            "a".to_owned()
        ))
    );
}

#[tokio::test]
async fn rebuild_projections_matches_incremental_apply() {
    let mut fixture = Fixture::new(Role::Hub).await;
    fixture.seed_typical_stream().await;
    // Add a second session (equivalent to a fork) to verify rebuilding multiple sessions
    let parent_session_id = fixture.session_id.clone();
    let forked_id = uuid_v7();
    fixture.session_id = forked_id.clone();
    let forked_created = fixture.event(UnifiedEventPayload::SessionCreated {
        node_id: NODE_ID.into(),
        project_id: PROJECT_ID.into(),
        project_name: "flexagent".to_owned(),
        local_path: "/home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth"
            .to_owned(),
        git_branch: Some("feat/auth".to_owned()),
        is_worktree: true,
        agent_id: "opencode2".to_owned(),
        parent_session_id: Some(parent_session_id.clone().into()),
        title: "Forked Session".to_owned(),
    });
    let forked_message = fixture.event(UnifiedEventPayload::UserMessage {
        text: "This is a forked session".to_owned(),
        attachments: vec![],
        client_source: ClientSource::Web,
        agent_message_id: None,
    });
    fixture
        .db
        .append_events(&[forked_created, forked_message])
        .await
        .expect("append forked session");

    let sessions_before = fixture
        .db
        .list_sessions(&SessionFilter::default())
        .await
        .expect("sessions before");
    let request_before = fixture
        .db
        .find_permission_request(&parent_session_id, "req-1")
        .await
        .expect("request before")
        .expect("exists");

    fixture.db.rebuild_projections().await.expect("rebuild");

    let sessions_after = fixture
        .db
        .list_sessions(&SessionFilter::default())
        .await
        .expect("sessions after");
    let request_after = fixture
        .db
        .find_permission_request(&parent_session_id, "req-1")
        .await
        .expect("request after")
        .expect("exists");

    assert_eq!(
        sessions_after, sessions_before,
        "a full rebuild must match incremental event application"
    );
    assert_eq!(request_after, request_before);

    // The fork source information is preserved
    let forked = fixture
        .db
        .get_session(&forked_id)
        .await
        .expect("get forked")
        .expect("exists");
    assert_eq!(
        forked
            .parent_session_id
            .as_ref()
            .map(fxg_protocol::SessionId::as_str),
        Some(parent_session_id.as_str())
    );
    assert!(forked.is_worktree);

    // Idempotent: rebuilding again changes nothing
    fixture
        .db
        .rebuild_projections()
        .await
        .expect("rebuild twice");
    let sessions_twice = fixture
        .db
        .list_sessions(&SessionFilter::default())
        .await
        .expect("sessions twice");
    assert_eq!(sessions_twice, sessions_after);
}

/// Drains every outbox page (production pages; tests want the whole log).
async fn drain_outbox(db: &Db) -> Vec<fxg_db::SessionOutboxBatch> {
    let mut batches: Vec<fxg_db::SessionOutboxBatch> = Vec::new();
    let mut cursor = None;
    loop {
        let page = db
            .extract_outbox_page(cursor.as_ref(), 100)
            .await
            .expect("outbox page");
        for mut batch in page.batches {
            match batches.last_mut() {
                Some(last) if last.session_id == batch.session_id => {
                    last.events.append(&mut batch.events);
                }
                _ => batches.push(batch),
            }
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => return batches,
        }
    }
}

#[tokio::test]
async fn outbox_watermark_extraction_and_ack() {
    let mut fixture = Fixture::new(Role::Node).await;
    let events = fixture.seed_typical_stream().await;

    // Until ACKed, all events are in the outbox (batched per session)
    let outbox = drain_outbox(&fixture.db).await;
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].session_id, fixture.session_id);
    assert_eq!(outbox[0].events.len(), events.len());
    assert_eq!(outbox[0].last_node_seq(), Some(8));
    assert_eq!(fixture.db.unsynced_event_count().await.expect("count"), 8);
    assert_eq!(fixture.db.last_synced_at().await.expect("synced at"), None);

    // ACKing up to the third event advances the watermark
    fixture
        .db
        .ack_watermark(&fixture.session_id, 3)
        .await
        .expect("ack");
    let outbox = drain_outbox(&fixture.db).await;
    assert_eq!(outbox[0].events.len(), 5);
    assert_eq!(outbox[0].events[0].node_seq, 4);
    assert_eq!(fixture.db.unsynced_event_count().await.expect("count"), 5);
    assert_eq!(
        fixture.db.last_synced_at().await.expect("synced at"),
        Some(events[2].created_at)
    );

    // A rewinding ACK is ignored (MAX update)
    fixture
        .db
        .ack_watermark(&fixture.session_id, 1)
        .await
        .expect("ack older");
    assert_eq!(fixture.db.unsynced_event_count().await.expect("count"), 5);

    // ACKing everything empties the outbox
    fixture
        .db
        .ack_watermark(&fixture.session_id, 8)
        .await
        .expect("ack all");
    assert!(drain_outbox(&fixture.db).await.is_empty());
    assert_eq!(fixture.db.unsynced_event_count().await.expect("count"), 0);

    // Range resend for ResyncRequest works regardless of the watermark
    let resend = fixture
        .db
        .extract_outbox_after(&fixture.session_id, 6, 100)
        .await
        .expect("resync");
    assert_eq!(resend.len(), 3);
    assert_eq!(resend[0].node_seq, 6);
    // from_node_seq = 1 resends everything including SessionCreated
    let full = fixture
        .db
        .extract_outbox_after(&fixture.session_id, 1, 100)
        .await
        .expect("full resync");
    assert_eq!(full.len(), 8);

    // ACK for an unknown session is an error
    let err = fixture
        .db
        .ack_watermark("missing-session", 1)
        .await
        .expect_err("unknown session");
    assert!(matches!(err, DbError::SessionNotFound(_)));
}

#[tokio::test]
async fn outbox_pages_bound_each_page_and_cover_every_event() {
    let mut fixture = Fixture::new(Role::Node).await;
    let first = fixture.seed_typical_stream().await;

    // A second session interleaves with the first in `(session_id, node_seq)` order.
    let second = SessionEventEnvelope {
        event_id: uuid_v7().into(),
        session_id: uuid_v7().into(),
        node_seq: 1,
        created_at: now_ms(),
        payload: session_created_payload(),
    };
    fixture
        .db
        .append_events(&[second])
        .await
        .expect("append second session");

    let mut cursor = None;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let page = fixture
            .db
            .extract_outbox_page(cursor.as_ref(), 3)
            .await
            .expect("page");
        let count: usize = page.batches.iter().map(|batch| batch.events.len()).sum();
        assert!(count <= 3, "a page must not exceed the limit");
        for batch in page.batches {
            for event in batch.events {
                seen.push((event.session_id.to_string(), event.node_seq));
            }
        }
        pages += 1;
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    assert!(pages > 1, "the test must exercise more than one page");
    assert_eq!(seen.len(), first.len() + 1);
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "pages must not repeat events");
    assert!(
        seen.windows(2).all(|pair| pair[0] < pair[1]),
        "pages must be in (session_id, node_seq) order"
    );
}

#[tokio::test]
async fn fts_search_matches_text_and_excludes_binary_events() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let user = fixture.event(UnifiedEventPayload::UserMessage {
        text: "認証エラーのデバッグをお願いします".to_owned(), // allow-japanese: CJK full-text search test
        attachments: vec![],
        client_source: ClientSource::Cli,
        agent_message_id: None,
    });
    let terminal = fixture.event(UnifiedEventPayload::TerminalOutput {
        terminal_id: "term-1".to_owned(),
        command: "cargo test".to_owned(),
        data_b64: "c2VjcmV0LXRva2VuLXZhbHVl".to_owned(), // "secret-token-value"
        exit_code: Some(0),
        subagent_id: None,
    });
    let message = fixture.event(UnifiedEventPayload::AgentMessage {
        message_id: "m-1".to_owned(),
        text: "Fixed the OpenCode2Driver connection".to_owned(),
        is_complete: true,
        subagent_id: None,
    });
    fixture
        .db
        .append_events(&[created, user, terminal, message])
        .await
        .expect("append");

    // Japanese text can be searched through FTS (search index synced by triggers)
    let hits = fixture
        .db
        .search("認証エラー", 10) // allow-japanese: CJK full-text search test
        .await
        .expect("search japanese");
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].event_type,
        fxg_protocol::common::EventType::UserMessage
    );
    assert!(hits[0].snippet.contains("認証エラー")); // allow-japanese: CJK full-text search test
    assert!(hits[0].snippet.contains('['));

    // Partial match on a code identifier
    let hits = fixture
        .db
        .search("OpenCode2Driver", 10)
        .await
        .expect("search identifier");
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].event_type,
        fxg_protocol::common::EventType::AgentMessage
    );

    // TerminalOutput (binary) is excluded from FTS
    let hits = fixture
        .db
        .search("c2VjcmV0LXRva2VuLXZhbHVl", 10)
        .await
        .expect("search base64");
    assert!(hits.is_empty(), "terminal output must be excluded from FTS");

    // A term that does not hit
    let hits = fixture
        .db
        .search("存在しない検索語", 10) // allow-japanese: CJK full-text search test
        .await
        .expect("search missing");
    assert!(hits.is_empty());

    // Queries shorter than 3 characters use the LIKE fallback
    let hits = fixture.db.search("認証", 10).await.expect("short query"); // allow-japanese: CJK short-query test
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("認証")); // allow-japanese: CJK short-query test

    // An empty query is an error
    assert!(matches!(
        fixture.db.search("   ", 10).await.expect_err("empty"),
        DbError::EmptySearchQuery
    ));

    // Search terms containing LIKE metacharacters do not error
    let hits = fixture.db.search("100%", 10).await.expect("like meta");
    assert!(hits.is_empty());
}

#[tokio::test]
async fn fts_index_stays_in_sync_on_regenerate_and_delete() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let message = fixture.event(UnifiedEventPayload::AgentMessage {
        message_id: "m-1".to_owned(),
        text: "Applied the migration".to_owned(),
        is_complete: true,
        subagent_id: None,
    });
    fixture
        .db
        .append_events(&[created, message])
        .await
        .expect("append");

    // Corrupt searchable_text, regenerate, then search again (UPDATE trigger check)
    sqlx::query("UPDATE session_events SET searchable_text = NULL")
        .execute(fixture.db.pool())
        .await
        .expect("corrupt searchable text");
    let hits = fixture
        .db
        .search("migration", 10)
        .await
        .expect("search after corrupt");
    assert!(hits.is_empty());

    let updated = fixture
        .db
        .regenerate_searchable_text()
        .await
        .expect("regenerate");
    assert_eq!(updated, 2);
    let hits = fixture
        .db
        .search("migration", 10)
        .await
        .expect("search after regenerate");
    assert_eq!(hits.len(), 1);

    // Rebuilding the whole FTS index from the content table gives the same result
    fixture.db.rebuild_fts_index().await.expect("rebuild fts");
    let hits = fixture
        .db
        .search("migration", 10)
        .await
        .expect("search after rebuild");
    assert_eq!(hits.len(), 1);

    // Deleting the session (CASCADE) keeps events and the search index in sync
    // (DELETE trigger check)
    sqlx::query("DELETE FROM sessions WHERE session_id = ?")
        .bind(&fixture.session_id)
        .execute(fixture.db.pool())
        .await
        .expect("delete session");
    let hits = fixture
        .db
        .search("migration", 10)
        .await
        .expect("search after delete");
    assert!(hits.is_empty(), "FTS index must follow cascade deletes");
}

#[tokio::test]
async fn session_events_after_paginates_by_cursor() {
    let mut fixture = Fixture::new(Role::Node).await;
    fixture.seed_typical_stream().await;

    let page1 = fixture
        .db
        .session_events_after(&fixture.session_id, 0, 2)
        .await
        .expect("page1");
    assert_eq!(page1.events.len(), 2);
    assert_eq!(page1.events[0].node_seq, 1);
    assert!(page1.cursor > 0);

    let page2 = fixture
        .db
        .session_events_after(&fixture.session_id, page1.cursor, 100)
        .await
        .expect("page2");
    assert_eq!(page2.events.len(), 6);
    assert_eq!(page2.events[0].node_seq, 3);

    let page3 = fixture
        .db
        .session_events_after(&fixture.session_id, page2.cursor, 100)
        .await
        .expect("page3");
    assert!(page3.events.is_empty());
    assert_eq!(page3.cursor, page2.cursor, "empty page keeps the cursor");

    // Event payloads are restored correctly
    let all = fixture
        .db
        .session_events_after(&fixture.session_id, 0, 100)
        .await
        .expect("all");
    assert!(matches!(
        all.events[0].payload,
        UnifiedEventPayload::SessionCreated { .. }
    ));
    assert!(matches!(
        all.events[5].payload,
        UnifiedEventPayload::UserMessage { .. }
    ));
}

#[tokio::test]
async fn node_and_hub_return_identical_client_responses() {
    let session_id = uuid_v7();
    let mut node_fixture = Fixture::with_session_id(Role::Node, session_id.clone()).await;
    let hub_fixture = Fixture::with_session_id(Role::Hub, session_id.clone()).await;

    let events = node_fixture.seed_typical_stream().await;
    hub_fixture
        .db
        .append_events(&events)
        .await
        .expect("hub append");

    let node_sessions = node_fixture
        .db
        .list_sessions(&SessionFilter::default())
        .await
        .expect("node sessions");
    let hub_sessions = hub_fixture
        .db
        .list_sessions(&SessionFilter::default())
        .await
        .expect("hub sessions");
    assert_eq!(node_sessions, hub_sessions);

    // Node projections match except for the registration time (last_seen_at)
    let normalize_nodes = |nodes: Vec<fxg_protocol::common::NodeSummary>| {
        nodes
            .into_iter()
            .map(|node| {
                (
                    node.node_id,
                    node.name,
                    node.os,
                    node.arch,
                    node.version,
                    node.is_ephemeral,
                    node.provisioner,
                    node.lifecycle_status,
                    node.is_online,
                    node.installed_agents,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        normalize_nodes(node_fixture.db.list_nodes().await.expect("node nodes")),
        normalize_nodes(hub_fixture.db.list_nodes().await.expect("hub nodes"))
    );
    // Project projections also match except for the registration time
    let normalize_projects = |projects: Vec<fxg_protocol::common::ProjectSummary>| {
        projects
            .into_iter()
            .map(|project| {
                (
                    project.project_id,
                    project.name,
                    project.canonical_git_url,
                    project
                        .bindings
                        .into_iter()
                        .map(|binding| {
                            (
                                binding.node_id,
                                binding.local_path,
                                binding.is_worktree,
                                binding.git_branch,
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        normalize_projects(
            node_fixture
                .db
                .list_projects()
                .await
                .expect("node projects")
        ),
        normalize_projects(hub_fixture.db.list_projects().await.expect("hub projects"))
    );
    assert_eq!(
        node_fixture
            .db
            .find_permission_request(&session_id, "req-1")
            .await
            .expect("node request"),
        hub_fixture
            .db
            .find_permission_request(&session_id, "req-1")
            .await
            .expect("hub request")
    );
    assert_eq!(
        node_fixture
            .db
            .session_events_after(&session_id, 0, 100)
            .await
            .expect("node events"),
        hub_fixture
            .db
            .session_events_after(&session_id, 0, 100)
            .await
            .expect("hub events")
    );
    assert_eq!(
        node_fixture
            .db
            .search("cargo test", 10)
            .await
            .expect("node search"),
        hub_fixture
            .db
            .search("cargo test", 10)
            .await
            .expect("hub search")
    );
}

#[tokio::test]
async fn session_filters_and_plan_projection() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let plan = fixture.event(UnifiedEventPayload::PlanUpdate {
        entries: vec![PlanEntry {
            id: "1".to_owned(),
            title: "Add tests".to_owned(),
            status: PlanEntryStatus::Pending,
        }],
        subagent_id: None,
    });
    let stopped = fixture.event(UnifiedEventPayload::StatusChanged {
        status: SessionStatus::Stopped,
        error_message: None,
    });
    fixture
        .db
        .append_events(&[created, plan, stopped])
        .await
        .expect("append");

    // Filters by project / node / status
    let by_project = fixture
        .db
        .list_sessions(&SessionFilter {
            project_id: Some(PROJECT_ID.into()),
            ..SessionFilter::default()
        })
        .await
        .expect("filter by project");
    assert_eq!(by_project.len(), 1);

    let by_other_project = fixture
        .db
        .list_sessions(&SessionFilter {
            project_id: Some("github.com/other/repo".into()),
            ..SessionFilter::default()
        })
        .await
        .expect("filter by other project");
    assert!(by_other_project.is_empty());

    let by_node = fixture
        .db
        .list_sessions(&SessionFilter {
            node_id: Some(NODE_ID.into()),
            ..SessionFilter::default()
        })
        .await
        .expect("filter by node");
    assert_eq!(by_node.len(), 1);

    let by_other_node = fixture
        .db
        .list_sessions(&SessionFilter {
            node_id: Some("other-node".into()),
            ..SessionFilter::default()
        })
        .await
        .expect("filter by other node");
    assert!(by_other_node.is_empty());

    let running_only = fixture
        .db
        .list_sessions(&SessionFilter {
            statuses: vec![SessionStatus::Running],
            ..SessionFilter::default()
        })
        .await
        .expect("filter by status");
    assert!(running_only.is_empty(), "session is stopped");

    let stopped_only = fixture
        .db
        .list_sessions(&SessionFilter {
            statuses: vec![SessionStatus::Stopped],
            ..SessionFilter::default()
        })
        .await
        .expect("filter stopped");
    assert_eq!(stopped_only.len(), 1);

    let limited = fixture
        .db
        .list_sessions(&SessionFilter {
            limit: Some(0),
            ..SessionFilter::default()
        })
        .await
        .expect("limit 0");
    assert!(limited.is_empty());
}

#[tokio::test]
async fn session_list_keyset_pagination() {
    let fixture = Fixture::new(Role::Node).await;
    let ids = [
        SessionId::from(uuid_v7()),
        SessionId::from(uuid_v7()),
        SessionId::from(uuid_v7()),
    ];
    let mut events = Vec::new();
    for (index, session_id) in ids.iter().enumerate() {
        events.push(SessionEventEnvelope {
            event_id: fxg_protocol::EventId::from(uuid_v7()),
            session_id: session_id.clone(),
            node_seq: 1,
            // Older sessions get older update times (index 0 is the oldest).
            created_at: 1_000 + index as i64,
            payload: session_created_payload(),
        });
    }
    fixture.db.append_events(&events).await.expect("append");

    let first = fixture
        .db
        .list_session_page(&SessionFilter {
            limit: Some(2),
            ..SessionFilter::default()
        })
        .await
        .expect("first page");
    assert_eq!(first.sessions.len(), 2);
    assert!(first.has_more);
    assert_eq!(first.sessions[0].session_id, ids[2]);
    assert_eq!(first.sessions[1].session_id, ids[1]);

    let last = first.sessions.last().expect("last session");
    let cursor = SessionCursor {
        updated_at: last.updated_at,
        session_id: last.session_id.clone(),
    };
    let second = fixture
        .db
        .list_session_page(&SessionFilter {
            limit: Some(2),
            cursor: Some(SessionCursor::decode(&cursor.encode()).expect("decode cursor")),
            ..SessionFilter::default()
        })
        .await
        .expect("second page");
    assert_eq!(second.sessions.len(), 1);
    assert!(!second.has_more);
    assert_eq!(second.sessions[0].session_id, ids[0]);

    // The cursor of the final page yields an empty page (clients stop at `has_more=false`).
    let last = second.sessions.last().expect("last session");
    let tail = fixture
        .db
        .list_session_page(&SessionFilter {
            limit: Some(2),
            cursor: Some(SessionCursor {
                updated_at: last.updated_at,
                session_id: last.session_id.clone(),
            }),
            ..SessionFilter::default()
        })
        .await
        .expect("tail page");
    assert!(tail.sessions.is_empty());
    assert!(!tail.has_more);
}

#[tokio::test]
async fn session_list_returns_per_project_cursors() {
    let fixture = Fixture::new(Role::Node).await;
    let project_a = PROJECT_ID;
    let project_b = "other.example/project-b";
    let entries = [
        (project_a, LOCAL_PATH, 1_000_i64, "a1"),
        (project_b, "/tmp/other", 1_500, "b1"),
        (project_a, LOCAL_PATH, 2_000, "a2"),
        (project_b, "/tmp/other", 2_500, "b2"),
        (project_a, LOCAL_PATH, 3_000, "a3"),
    ];
    for (project_id, local_path, created_at, title) in entries {
        fixture
            .db
            .append_events(&[SessionEventEnvelope {
                event_id: fxg_protocol::EventId::from(uuid_v7()),
                session_id: SessionId::from(uuid_v7()),
                node_seq: 1,
                created_at,
                payload: UnifiedEventPayload::SessionCreated {
                    node_id: NODE_ID.into(),
                    project_id: project_id.into(),
                    project_name: project_id.to_owned(),
                    local_path: local_path.to_owned(),
                    git_branch: Some("main".to_owned()),
                    is_worktree: false,
                    agent_id: "opencode2".to_owned(),
                    parent_session_id: None,
                    title: title.to_owned(),
                },
            }])
            .await
            .expect("append");
    }

    let first = fixture
        .db
        .list_session_page(&SessionFilter {
            limit: Some(2),
            ..SessionFilter::default()
        })
        .await
        .expect("first page");
    assert_eq!(
        first
            .sessions
            .iter()
            .map(|session| session.title.as_str())
            .collect::<Vec<_>>(),
        ["a3", "b2"]
    );
    assert!(first.has_more);
    assert_eq!(first.project_cursors.len(), 2);
    let cursor_a = first
        .project_cursors
        .iter()
        .find(|cursor| cursor.project_id.as_str() == project_a)
        .expect("project A cursor");
    let cursor_b = first
        .project_cursors
        .iter()
        .find(|cursor| cursor.project_id.as_str() == project_b)
        .expect("project B cursor");
    assert!(cursor_a.next_cursor.is_some());
    assert!(cursor_b.next_cursor.is_some());

    // Continuing project A must return only A's older sessions.
    let a_page = fixture
        .db
        .list_session_page(&SessionFilter {
            project_id: Some(project_a.into()),
            limit: Some(10),
            cursor: cursor_a.next_cursor.clone(),
            ..SessionFilter::default()
        })
        .await
        .expect("project A page");
    assert_eq!(
        a_page
            .sessions
            .iter()
            .map(|session| session.title.as_str())
            .collect::<Vec<_>>(),
        ["a2", "a1"]
    );
    assert!(!a_page.has_more);
    assert_eq!(a_page.project_cursors.len(), 1);
    assert!(
        a_page.project_cursors[0].next_cursor.is_none(),
        "project A is fully loaded"
    );

    // Same for project B.
    let b_page = fixture
        .db
        .list_session_page(&SessionFilter {
            project_id: Some(project_b.into()),
            limit: Some(10),
            cursor: cursor_b.next_cursor.clone(),
            ..SessionFilter::default()
        })
        .await
        .expect("project B page");
    assert_eq!(
        b_page
            .sessions
            .iter()
            .map(|session| session.title.as_str())
            .collect::<Vec<_>>(),
        ["b1"]
    );
    assert!(!b_page.has_more);
    assert!(b_page.project_cursors[0].next_cursor.is_none());
}

#[tokio::test]
async fn session_list_has_more_is_false_at_exact_limit() {
    let fixture = Fixture::new(Role::Node).await;
    let ids = [SessionId::from(uuid_v7()), SessionId::from(uuid_v7())];
    let events = ids
        .iter()
        .enumerate()
        .map(|(index, session_id)| SessionEventEnvelope {
            event_id: fxg_protocol::EventId::from(uuid_v7()),
            session_id: session_id.clone(),
            node_seq: 1,
            created_at: 1_000 + index as i64,
            payload: session_created_payload(),
        })
        .collect::<Vec<_>>();
    fixture.db.append_events(&events).await.expect("append");

    let page = fixture
        .db
        .list_session_page(&SessionFilter {
            limit: Some(2),
            ..SessionFilter::default()
        })
        .await
        .expect("page");
    assert_eq!(page.sessions.len(), 2);
    assert!(
        !page.has_more,
        "a page exactly at the limit is not truncated"
    );
}

#[tokio::test]
async fn session_list_project_cursor_is_none_for_a_fully_loaded_project() {
    let fixture = Fixture::new(Role::Node).await;
    let project_a = PROJECT_ID;
    let project_b = "other.example/project-b";
    let entries = [
        (project_a, LOCAL_PATH, 1_000_i64, "a1"),
        (project_b, "/tmp/other", 2_000, "b1"),
        (project_a, LOCAL_PATH, 3_000, "a2"),
    ];
    for (project_id, local_path, created_at, title) in entries {
        fixture
            .db
            .append_events(&[SessionEventEnvelope {
                event_id: fxg_protocol::EventId::from(uuid_v7()),
                session_id: SessionId::from(uuid_v7()),
                node_seq: 1,
                created_at,
                payload: UnifiedEventPayload::SessionCreated {
                    node_id: NODE_ID.into(),
                    project_id: project_id.into(),
                    project_name: project_id.to_owned(),
                    local_path: local_path.to_owned(),
                    git_branch: Some("main".to_owned()),
                    is_worktree: false,
                    agent_id: "opencode2".to_owned(),
                    parent_session_id: None,
                    title: title.to_owned(),
                },
            }])
            .await
            .expect("append");
    }

    let page = fixture
        .db
        .list_session_page(&SessionFilter {
            limit: Some(2),
            ..SessionFilter::default()
        })
        .await
        .expect("page");
    assert_eq!(
        page.sessions
            .iter()
            .map(|session| session.title.as_str())
            .collect::<Vec<_>>(),
        ["a2", "b1"]
    );
    assert!(page.has_more, "a1 is older than the page");

    let cursor_a = page
        .project_cursors
        .iter()
        .find(|cursor| cursor.project_id.as_str() == project_a)
        .expect("project A cursor");
    let cursor_b = page
        .project_cursors
        .iter()
        .find(|cursor| cursor.project_id.as_str() == project_b)
        .expect("project B cursor");
    assert!(
        cursor_a.next_cursor.is_some(),
        "project A has an older session"
    );
    assert!(
        cursor_b.next_cursor.is_none(),
        "project B is fully loaded even though the global list has more"
    );
}

#[tokio::test]
async fn lightweight_session_queries_match_the_session_list() {
    let fixture = Fixture::new(Role::Node).await;
    let running_session = SessionId::from(uuid_v7());
    let archived_session = SessionId::from(uuid_v7());

    fixture
        .db
        .append_events(&[
            SessionEventEnvelope {
                event_id: fxg_protocol::EventId::from(uuid_v7()),
                session_id: running_session.clone(),
                node_seq: 1,
                created_at: 1_000,
                payload: session_created_payload(),
            },
            SessionEventEnvelope {
                event_id: fxg_protocol::EventId::from(uuid_v7()),
                session_id: running_session.clone(),
                node_seq: 2,
                created_at: 1_100,
                payload: UnifiedEventPayload::SessionAgentBound {
                    agent_session_id: "agent-session-1".to_owned(),
                },
            },
            SessionEventEnvelope {
                event_id: fxg_protocol::EventId::from(uuid_v7()),
                session_id: running_session.clone(),
                node_seq: 3,
                created_at: 1_200,
                payload: UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Running,
                    error_message: None,
                },
            },
            SessionEventEnvelope {
                event_id: fxg_protocol::EventId::from(uuid_v7()),
                session_id: archived_session.clone(),
                node_seq: 1,
                created_at: 2_000,
                payload: UnifiedEventPayload::SessionCreated {
                    node_id: NODE_ID.into(),
                    project_id: "other.example/project-b".into(),
                    project_name: "project-b".to_owned(),
                    local_path: "/tmp/other".to_owned(),
                    git_branch: None,
                    is_worktree: false,
                    agent_id: "opencode2".to_owned(),
                    parent_session_id: None,
                    title: "archived".to_owned(),
                },
            },
            SessionEventEnvelope {
                event_id: fxg_protocol::EventId::from(uuid_v7()),
                session_id: archived_session.clone(),
                node_seq: 2,
                created_at: 2_100,
                payload: UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Stopped,
                    error_message: None,
                },
            },
            SessionEventEnvelope {
                event_id: fxg_protocol::EventId::from(uuid_v7()),
                session_id: archived_session.clone(),
                node_seq: 3,
                created_at: 2_200,
                payload: UnifiedEventPayload::SessionArchived { archived: true },
            },
        ])
        .await
        .expect("append");

    // The default list hides the archived session.
    assert_eq!(
        fixture
            .db
            .count_sessions(&SessionFilter::default())
            .await
            .expect("count"),
        1
    );
    assert_eq!(
        fixture
            .db
            .count_sessions(&SessionFilter {
                include_archived: true,
                ..SessionFilter::default()
            })
            .await
            .expect("count archived"),
        2
    );
    assert_eq!(
        fixture
            .db
            .count_sessions(&SessionFilter {
                statuses: vec![SessionStatus::Running],
                ..SessionFilter::default()
            })
            .await
            .expect("count running"),
        1
    );

    let ids = fixture
        .db
        .list_session_ids(&SessionFilter {
            include_archived: true,
            ..SessionFilter::default()
        })
        .await
        .expect("session ids");
    assert_eq!(ids.len(), 2);

    let sync_states = fixture
        .db
        .list_session_sync_states(&SessionFilter {
            include_archived: true,
            limit: Some(1),
            ..SessionFilter::default()
        })
        .await
        .expect("sync states");
    assert_eq!(sync_states.len(), 1);
    assert_eq!(sync_states[0].last_node_seq, 3);

    let mut paths = fixture
        .db
        .list_session_local_paths(&SessionFilter {
            node_id: Some(NODE_ID.into()),
            include_archived: true,
            ..SessionFilter::default()
        })
        .await
        .expect("local paths");
    paths.sort();
    assert_eq!(paths, vec![LOCAL_PATH.to_owned(), "/tmp/other".to_owned()]);

    let bindings = fixture
        .db
        .list_agent_session_bindings()
        .await
        .expect("agent bindings");
    assert_eq!(
        bindings,
        vec![(
            "opencode2".to_owned(),
            "agent-session-1".to_owned(),
            running_session
        )]
    );
}

#[tokio::test]
async fn registration_upserts_and_audit_logs() {
    let fixture = Fixture::new(Role::Hub).await;

    // Update node information
    fixture
        .db
        .upsert_node(
            &NodeRecord::new(NODE_ID, "Renamed Node", "linux", "aarch64", "0.2.0")
                .with_agents(vec!["opencode2".to_owned(), "antigravity-acp".to_owned()]),
        )
        .await
        .expect("update node");
    let nodes = fixture.db.list_nodes().await.expect("list nodes");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "Renamed Node");
    assert_eq!(nodes[0].installed_agents.len(), 2);
    assert!(nodes[0].is_online);

    // Issue and revoke the node_token hash
    fixture
        .db
        .set_node_token_hash(NODE_ID, Some("sha256-hash"))
        .await
        .expect("issue token");
    let (hash, issued_at): (Option<String>, Option<i64>) =
        sqlx::query_as("SELECT token_hash, token_issued_at FROM nodes WHERE node_id = ?")
            .bind(NODE_ID)
            .fetch_one(fixture.db.pool())
            .await
            .expect("read token");
    assert_eq!(hash.as_deref(), Some("sha256-hash"));
    assert!(issued_at.is_some());

    fixture
        .db
        .set_node_token_hash(NODE_ID, None)
        .await
        .expect("revoke token");
    let hash: Option<String> = sqlx::query_scalar("SELECT token_hash FROM nodes WHERE node_id = ?")
        .bind(NODE_ID)
        .fetch_one(fixture.db.pool())
        .await
        .expect("read token");
    assert!(hash.is_none(), "revoked token hash must be cleared");

    // Update the online state
    fixture
        .db
        .set_node_online(NODE_ID, false)
        .await
        .expect("offline");
    let nodes = fixture.db.list_nodes().await.expect("list nodes");
    assert!(!nodes[0].is_online);

    // Update the binding (branch change, path missing)
    fixture
        .db
        .upsert_project_binding(&ProjectBindingRecord {
            project_id: PROJECT_ID.into(),
            node_id: NODE_ID.into(),
            local_path: LOCAL_PATH.to_owned(),
            is_worktree: false,
            path_exists: false,
            git_branch: Some("feat/rebase".to_owned()),
        })
        .await
        .expect("update binding");
    let projects = fixture.db.list_projects().await.expect("projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].bindings.len(), 1);
    assert_eq!(
        projects[0].bindings[0].git_branch.as_deref(),
        Some("feat/rebase")
    );
    assert!(
        !projects[0].bindings[0].path_exists,
        "path_exists must round-trip through projections"
    );

    // Audit log
    let mut record = AuditLogRecord::new(actions::PERMISSION_RESOLVED, "192.168.1.10", "token:web");
    record.session_id = Some("s-1".to_owned());
    record.node_id = Some(NODE_ID.to_owned());
    record.client_user_agent = Some("Mozilla/5.0".to_owned());
    record.details = serde_json::json!({ "selected_option_id": "allow_once" }).into();
    let id = fixture
        .db
        .append_audit_log(&record)
        .await
        .expect("audit log");
    assert!(id > 0);

    let logs = fixture.db.audit_logs(10).await.expect("audit logs");
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].action, actions::PERMISSION_RESOLVED);
    assert_eq!(logs[0].client_ip, "192.168.1.10");
    assert_eq!(
        logs[0].details.as_value()["selected_option_id"],
        "allow_once"
    );

    let limited = fixture.db.audit_logs(1).await.expect("audit logs");
    assert_eq!(limited.len(), 1);

    // Operations on an unknown node are errors
    assert!(matches!(
        fixture
            .db
            .set_node_online("missing", true)
            .await
            .expect_err("missing node"),
        DbError::NodeNotFound(_)
    ));
}

#[tokio::test]
async fn default_local_path_prefers_recent_existing_binding() {
    let fixture = Fixture::new(Role::Hub).await;

    // Add a Worktree binding (almost the same time as the main repo → tie-break favors main)
    fixture
        .db
        .upsert_project_binding(&ProjectBindingRecord {
            project_id: PROJECT_ID.into(),
            node_id: NODE_ID.into(),
            local_path: "/home/nazo/src/flexagent-wt".to_owned(),
            is_worktree: true,
            path_exists: true,
            git_branch: Some("feat/x".to_owned()),
        })
        .await
        .expect("worktree binding");
    // Align the last-used time of both bindings to verify the tie-break
    sqlx::query("UPDATE project_node_bindings SET last_used_at = 1000")
        .execute(fixture.db.pool())
        .await
        .expect("align last_used_at");
    let path = fixture
        .db
        .resolve_default_local_path(&ProjectId::from(PROJECT_ID), &NodeId::from(NODE_ID))
        .await
        .expect("resolve default");
    assert_eq!(
        path.as_deref(),
        Some(LOCAL_PATH),
        "the main repository wins on ties"
    );

    // Aging the main repository's last use makes the most recently used Worktree the default
    sqlx::query("UPDATE project_node_bindings SET last_used_at = 1 WHERE local_path = ?")
        .bind(LOCAL_PATH)
        .execute(fixture.db.pool())
        .await
        .expect("age main binding");
    let path = fixture
        .db
        .resolve_default_local_path(&ProjectId::from(PROJECT_ID), &NodeId::from(NODE_ID))
        .await
        .expect("resolve default");
    assert_eq!(
        path.as_deref(),
        Some("/home/nazo/src/flexagent-wt"),
        "the most recently used Worktree wins"
    );

    // A non-existent binding is not used as default (falls back to an older existing binding)
    sqlx::query("UPDATE project_node_bindings SET path_exists = 0 WHERE local_path = ?")
        .bind("/home/nazo/src/flexagent-wt")
        .execute(fixture.db.pool())
        .await
        .expect("mark worktree missing");
    let path = fixture
        .db
        .resolve_default_local_path(&ProjectId::from(PROJECT_ID), &NodeId::from(NODE_ID))
        .await
        .expect("resolve default");
    assert_eq!(
        path.as_deref(),
        Some(LOCAL_PATH),
        "falls back to an existing binding"
    );

    // With no existing bindings, returns the most recently used one (the UI shows a warning)
    sqlx::query("UPDATE project_node_bindings SET path_exists = 0")
        .execute(fixture.db.pool())
        .await
        .expect("mark all missing");
    let path = fixture
        .db
        .resolve_default_local_path(&ProjectId::from(PROJECT_ID), &NodeId::from(NODE_ID))
        .await
        .expect("resolve default");
    assert_eq!(path.as_deref(), Some("/home/nazo/src/flexagent-wt"));

    // None when there are no bindings
    let path = fixture
        .db
        .resolve_default_local_path(
            &ProjectId::from("github.com/nazo6/none"),
            &NodeId::from(NODE_ID),
        )
        .await
        .expect("resolve default");
    assert!(path.is_none());
}

#[tokio::test]
async fn file_backed_db_persists_across_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("node").join("node.db");
    let session_id = uuid_v7();

    let db = Db::open(&path, Role::Node).await.expect("open file db");
    db.upsert_node(&NodeRecord::new(
        NODE_ID,
        "Test Node",
        "macos",
        "aarch64",
        "0.1.0",
    ))
    .await
    .expect("upsert node");
    db.upsert_project(&ProjectRecord {
        project_id: PROJECT_ID.into(),
        name: "flexagent".to_owned(),
        canonical_git_url: None,
        identity: fxg_protocol::settings::ProjectIdentity::local(),
    })
    .await
    .expect("upsert project");
    db.append_event(&SessionEventEnvelope {
        event_id: uuid_v7().into(),
        session_id: session_id.clone().into(),
        node_seq: 1,
        created_at: now_ms(),
        payload: session_created_payload(),
    })
    .await
    .expect("append");
    db.close().await;

    let db = Db::open(&path, Role::Node).await.expect("reopen");
    let session = db
        .get_session(&session_id)
        .await
        .expect("get session")
        .expect("session persisted");
    assert_eq!(session.title, "New Session");
    assert_eq!(count_events(&db, &session_id).await, 1);
    db.close().await;
}

#[tokio::test]
async fn copy_fork_conversation_mirrors_conversation_events_only() {
    let mut source = Fixture::new(Role::Node).await;
    let created = source.session_created();
    let user = source.event(UnifiedEventPayload::UserMessage {
        text: "parent question".to_owned(),
        attachments: vec![],
        client_source: ClientSource::Web,
        agent_message_id: None,
    });
    let user_created_at = user.created_at;
    let answer = source.event(UnifiedEventPayload::AgentMessage {
        message_id: "parent-message".to_owned(),
        text: "parent answer".to_owned(),
        is_complete: true,
        subagent_id: None,
    });
    // Session-scoped meta event: must not be mirrored into the fork.
    let status = source.event(UnifiedEventPayload::StatusChanged {
        status: SessionStatus::Idle,
        error_message: None,
    });
    let fork_snapshot = source.seq;
    source
        .db
        .append_events(&[created, user, answer, status])
        .await
        .expect("seed source");

    // The fork's own SessionCreated already exists (node_seq = 1).
    let target_id = uuid_v7();
    source
        .db
        .append_event(&SessionEventEnvelope {
            event_id: uuid_v7().into(),
            session_id: target_id.clone().into(),
            node_seq: 1,
            created_at: now_ms(),
            payload: session_created_payload(),
        })
        .await
        .expect("target session");

    // An event recorded after the fork snapshot belongs to a later turn.
    source
        .db
        .append_next_event(
            &source.session_id,
            UnifiedEventPayload::UserMessage {
                text: "after the fork".to_owned(),
                attachments: vec![],
                client_source: ClientSource::Web,
                agent_message_id: None,
            },
        )
        .await
        .expect("later source event");

    let copied = source
        .db
        .copy_fork_conversation(&source.session_id, &target_id, fork_snapshot)
        .await
        .expect("copy fork conversation");
    assert_eq!(copied.len(), 2, "only conversation events are copied");
    assert_eq!(copied[0].envelope.node_seq, 2);
    assert_eq!(copied[1].envelope.node_seq, 3);

    let batch = source
        .db
        .session_events_after(&target_id, 0, 100)
        .await
        .expect("target events");
    let kinds: Vec<EventType> = batch
        .events
        .iter()
        .map(|e| e.payload.event_type())
        .collect();
    assert_eq!(
        kinds,
        vec![
            EventType::SessionCreated,
            EventType::UserMessage,
            EventType::AgentMessage
        ]
    );
    let node_seqs: Vec<u64> = batch.events.iter().map(|e| e.node_seq).collect();
    assert_eq!(node_seqs, vec![1, 2, 3]);
    // The original event time is preserved so the timeline keeps its order.
    assert_eq!(batch.events[1].created_at, user_created_at);

    let target = source
        .db
        .get_session(&target_id)
        .await
        .expect("get target")
        .expect("target row");
    assert_eq!(target.last_node_seq, 3);
}

#[tokio::test]
async fn append_next_event_allocates_gap_free_sequences() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    fixture
        .db
        .append_event(&created)
        .await
        .expect("session created");

    let second = fixture
        .db
        .append_next_event(
            &fixture.session_id,
            UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            },
        )
        .await
        .expect("second event");
    assert_eq!(second.envelope.node_seq, 2);

    let third = fixture
        .db
        .append_next_event(
            &fixture.session_id,
            UnifiedEventPayload::UserMessage {
                text: "Continue".to_owned(),
                attachments: vec![],
                client_source: ClientSource::Cli,
                agent_message_id: None,
            },
        )
        .await
        .expect("third event");
    assert_eq!(third.envelope.node_seq, 3);

    let summary = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(summary.last_node_seq, 3);
    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 3);

    // Allocation for an unknown session is an error
    let err = fixture
        .db
        .append_next_event(
            "missing-session",
            UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Idle,
                error_message: None,
            },
        )
        .await
        .expect_err("unknown session");
    assert!(matches!(err, DbError::SessionNotFound(_)));

    // Events that must not be persisted are rejected (keystroke)
    let err = fixture
        .db
        .append_next_event(
            &fixture.session_id,
            UnifiedEventPayload::TerminalInput {
                terminal_id: "t1".to_owned(),
                data_b64: "aGk=".to_owned(),
            },
        )
        .await
        .expect_err("terminal input");
    assert!(matches!(
        err,
        DbError::NonPersistableEvent(fxg_protocol::common::EventType::TerminalInput)
    ));

    // Rejection does not consume node_seq (the transaction is rolled back)
    let fourth = fixture
        .db
        .append_next_event(
            &fixture.session_id,
            UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Idle,
                error_message: None,
            },
        )
        .await
        .expect("fourth event");
    assert_eq!(fourth.envelope.node_seq, 4);
}

#[tokio::test]
async fn non_persistable_events_are_rejected_on_batch_append() {
    let mut fixture = Fixture::new(Role::Node).await;
    let created = fixture.session_created();
    let partial = fixture.event(UnifiedEventPayload::AgentMessage {
        message_id: "m1".to_owned(),
        text: "partial".to_owned(),
        is_complete: false,
        subagent_id: None,
    });

    let err = fixture
        .db
        .append_events(&[created.clone(), partial.clone()])
        .await
        .expect_err("incomplete message");
    assert!(matches!(
        err,
        DbError::NonPersistableEvent(fxg_protocol::common::EventType::AgentMessage)
    ));

    // The whole batch is rolled back (SessionCreated is not inserted either)
    assert!(
        fixture
            .db
            .get_session(&fixture.session_id)
            .await
            .expect("get")
            .is_none()
    );

    // A completed event can be appended
    let mut complete = partial;
    complete.payload = UnifiedEventPayload::AgentMessage {
        message_id: "m1".to_owned(),
        text: "complete".to_owned(),
        is_complete: true,
        subagent_id: None,
    };
    fixture
        .db
        .append_events(&[created, complete])
        .await
        .expect("complete message");
    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 2);
}

#[tokio::test]
async fn session_archiving_hides_from_default_list_and_is_reversible() {
    let mut fixture = Fixture::new(Role::Node).await;
    fixture.seed_typical_stream().await;

    let archived = fixture.event(UnifiedEventPayload::SessionArchived { archived: true });
    fixture.db.append_event(&archived).await.expect("archive");

    // The event log is retained (archiving is a reversible visibility flag)
    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 9);
    let summary = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(summary.archived_at, Some(archived.created_at));

    // Excluded from the default list but retrievable with `include_archived`
    assert!(
        fixture
            .db
            .list_sessions(&SessionFilter::default())
            .await
            .expect("default list")
            .is_empty()
    );
    let with_archived = fixture
        .db
        .list_sessions(&SessionFilter {
            include_archived: true,
            ..SessionFilter::default()
        })
        .await
        .expect("list with archived");
    assert_eq!(with_archived.len(), 1);
    assert_eq!(with_archived[0].archived_at, Some(archived.created_at));

    // Archive state is preserved through a full rebuild (event-derived projection)
    fixture.db.rebuild_projections().await.expect("rebuild");
    let rebuilt = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get rebuilt")
        .expect("exists");
    assert_eq!(rebuilt.archived_at, Some(archived.created_at));

    // Can be restored
    let restored = fixture.event(UnifiedEventPayload::SessionArchived { archived: false });
    fixture.db.append_event(&restored).await.expect("unarchive");
    let summary = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(summary.archived_at, None);
    assert_eq!(
        fixture
            .db
            .list_sessions(&SessionFilter::default())
            .await
            .expect("list")
            .len(),
        1
    );
}

#[tokio::test]
async fn session_deletion_purges_content_and_keeps_tombstone() {
    let mut fixture = Fixture::new(Role::Node).await;
    fixture.seed_typical_stream().await;
    // Sync all events before deleting (only the tombstone goes into the outbox)
    fixture
        .db
        .ack_watermark(&fixture.session_id, 8)
        .await
        .expect("ack");

    let deleted = fixture.event(UnifiedEventPayload::SessionDeleted {});
    fixture.db.append_event(&deleted).await.expect("delete");

    // Body events and approval history are purged, leaving only the tombstone
    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 1);
    assert!(
        fixture
            .db
            .find_permission_request(&fixture.session_id, "req-1")
            .await
            .expect("find")
            .is_none()
    );
    assert!(
        fixture
            .db
            .get_session(&fixture.session_id)
            .await
            .expect("get")
            .is_none(),
        "a deleted session cannot be retrieved (tombstone)"
    );
    assert!(
        fixture
            .db
            .list_sessions(&SessionFilter {
                include_archived: true,
                ..SessionFilter::default()
            })
            .await
            .expect("list")
            .is_empty()
    );

    // Sync: only the tombstone goes into the outbox
    let outbox = drain_outbox(&fixture.db).await;
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].events.len(), 1);
    assert!(matches!(
        outbox[0].events[0].payload,
        UnifiedEventPayload::SessionDeleted {}
    ));
    assert_eq!(outbox[0].last_node_seq(), Some(9));

    // Events racing with deletion (e.g. turn completion during stop) are discarded
    let straggler = SessionEventEnvelope {
        event_id: uuid_v7().into(),
        session_id: fixture.session_id.clone().into(),
        node_seq: 10,
        created_at: now_ms(),
        payload: UnifiedEventPayload::UserMessage {
            text: "event received after deletion".to_owned(),
            attachments: vec![],
            client_source: ClientSource::Web,
            agent_message_id: None,
        },
    };
    let outcome = fixture
        .db
        .append_event(&straggler)
        .await
        .expect("straggler");
    assert!(
        !outcome.inserted,
        "appends to a deleted session are discarded"
    );
    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 1);

    // Deletion state is preserved through a full rebuild
    fixture.db.rebuild_projections().await.expect("rebuild");
    assert!(
        fixture
            .db
            .get_session(&fixture.session_id)
            .await
            .expect("get rebuilt")
            .is_none()
    );
    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 1);
}

#[tokio::test]
async fn session_deleted_for_unknown_session_is_noop() {
    // A tombstone for an unknown session (e.g. from a resync response) must not cause an
    // FK violation
    let db = Db::open_in_memory(Role::Hub).await.expect("open db");
    let envelope = SessionEventEnvelope {
        event_id: uuid_v7().into(),
        session_id: uuid_v7().into(),
        node_seq: 5,
        created_at: now_ms(),
        payload: UnifiedEventPayload::SessionDeleted {},
    };
    let outcome = db.append_event(&envelope).await.expect("noop");
    assert!(!outcome.inserted);
}

#[tokio::test]
async fn custom_agent_capabilities_roundtrip() {
    use fxg_protocol::settings::{AgentAcpCapabilities, CustomAgentConfig};

    let db = Db::open_in_memory(Role::Node).await.expect("open db");
    db.upsert_custom_agent(
        "mine",
        &CustomAgentConfig {
            command: "mine".to_owned(),
            capabilities: AgentAcpCapabilities { subagents: false },
            ..Default::default()
        },
    )
    .await
    .expect("custom agent");

    let agents = db.custom_agents().await.expect("agents");
    assert!(!agents["mine"].capabilities.subagents);

    // A definition written without the field still reads as the default.
    sqlx::query(r#"UPDATE custom_agents SET capabilities_json = '{}' WHERE agent_id = 'mine'"#)
        .execute(db.pool())
        .await
        .expect("rewrite capabilities");
    let agents = db.custom_agents().await.expect("agents");
    assert!(agents["mine"].capabilities.subagents);
}
