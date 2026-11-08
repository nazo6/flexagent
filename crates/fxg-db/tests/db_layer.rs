//! Integration tests for `fxg-db`.
//!
//! Verification of the Phase 1 completion criteria:
//! - Migration application (single schema + FTS5 + triggers)
//! - Idempotency of event application (`INSERT OR IGNORE` + same-transaction projections)
//! - Projection rebuild matches event application results
//! - Watermark-based outbox extraction and ACK
//! - FTS5 full-text search (via triggers, `searchable_text` exclusion rules, LIKE fallback)
//! - Local Node (`node.db`) and Central Server (`server.db`) return identical response types

use fxg_db::audit::actions;
use fxg_db::{
    AuditLogRecord, Db, DbError, DbRole, NodeRecord, ProjectBindingRecord, ProjectRecord,
    SessionFilter,
};
use fxg_protocol::common::{
    ClientSource, CommandInfo, ElicitationAction, ElicitationMode, ElicitationRequestStatus,
    ModeInfo, PermissionOption, PermissionOptionKind, PermissionRequestStatus, PlanEntry,
    PlanEntryStatus, ResolvedBy, SessionStatus, UsageCost,
};
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use fxg_protocol::util::{now_ms, uuid_v7};

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
    async fn new(role: DbRole) -> Self {
        let session_id = uuid_v7();
        Self::with_session_id(role, session_id).await
    }

    async fn with_session_id(role: DbRole, session_id: String) -> Self {
        let db = Db::open_in_memory(role).await.expect("open in-memory db");
        db.upsert_node(
            &NodeRecord::new(NODE_ID, "Test Node", "linux", "aarch64", "0.1.0")
                .with_agents(vec!["opencode2".to_owned()]),
        )
        .await
        .expect("upsert node");
        db.upsert_project(&ProjectRecord {
            project_id: PROJECT_ID.to_owned(),
            name: "flexagent".to_owned(),
            canonical_git_url: Some("git@github.com:nazo6/flexagent.git".to_owned()),
        })
        .await
        .expect("upsert project");
        db.upsert_project_binding(&ProjectBindingRecord {
            project_id: PROJECT_ID.to_owned(),
            node_id: NODE_ID.to_owned(),
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
            event_id: uuid_v7(),
            session_id: self.session_id.clone(),
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
            }),
            self.event(UnifiedEventPayload::UserMessage {
                text: "Fix the authentication error".to_owned(),
                attachments: vec![],
                client_source: ClientSource::Cli,
                snapshot_tree_hash: Some("b".repeat(40)),
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
        node_id: NODE_ID.to_owned(),
        project_id: PROJECT_ID.to_owned(),
        project_name: "flexagent".to_owned(),
        local_path: LOCAL_PATH.to_owned(),
        git_branch: Some("main".to_owned()),
        is_worktree: false,
        agent_id: "opencode2".to_owned(),
        parent_session_id: None,
        fork_from_node_seq: None,
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
    let db = Db::open_in_memory(DbRole::Node).await.expect("migrate");

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
    let mut fixture = Fixture::new(DbRole::Node).await;
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
    duplicate_seq.event_id = uuid_v7();
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
async fn event_application_updates_session_projections() {
    let mut fixture = Fixture::new(DbRole::Node).await;
    fixture.seed_typical_stream().await;

    let summary = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get session")
        .expect("session exists");
    assert_eq!(summary.project_id, PROJECT_ID);
    assert_eq!(summary.node_id, NODE_ID);
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
        .find_permission_request("req-1")
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
    let mut fixture = Fixture::new(DbRole::Node).await;
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
async fn usage_projection_tracks_latest_and_rebuilds() {
    let mut fixture = Fixture::new(DbRole::Node).await;
    let created = fixture.session_created();
    let first = fixture.event(UnifiedEventPayload::UsageUpdated {
        used_tokens: 1_000,
        context_size: 100_000,
        cost: None,
    });
    // The second update fully overwrites the first (keeps the latest, not cumulative)
    let second = fixture.event(UnifiedEventPayload::UsageUpdated {
        used_tokens: 53_000,
        context_size: 200_000,
        cost: Some(UsageCost {
            amount: 0.045,
            currency: "USD".to_owned(),
        }),
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
    let mut fixture = Fixture::new(DbRole::Node).await;
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
        .find_permission_request("req-reject")
        .await
        .expect("find")
        .expect("exists");
    assert_eq!(request.status, PermissionRequestStatus::Rejected);
    assert_eq!(request.resolved_by, Some(ResolvedBy::AndroidPush));
}

#[tokio::test]
async fn elicitation_projection_tracks_pending_and_resolved() {
    let mut fixture = Fixture::new(DbRole::Node).await;
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
        .find_elicitation_request("elic-1")
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
        .find_elicitation_request("elic-1")
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
    let mut fixture = Fixture::new(DbRole::Hub).await;
    fixture.seed_typical_stream().await;
    // Add a second session (equivalent to a fork) to verify rebuilding multiple sessions
    let parent_session_id = fixture.session_id.clone();
    let forked_id = uuid_v7();
    fixture.session_id = forked_id.clone();
    let forked_created = fixture.event(UnifiedEventPayload::SessionCreated {
        node_id: NODE_ID.to_owned(),
        project_id: PROJECT_ID.to_owned(),
        project_name: "flexagent".to_owned(),
        local_path: "/home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth"
            .to_owned(),
        git_branch: Some("feat/auth".to_owned()),
        is_worktree: true,
        agent_id: "opencode2".to_owned(),
        parent_session_id: Some(parent_session_id.clone()),
        fork_from_node_seq: Some(6),
        title: "Forked Session".to_owned(),
    });
    let forked_message = fixture.event(UnifiedEventPayload::UserMessage {
        text: "This is a forked session".to_owned(),
        attachments: vec![],
        client_source: ClientSource::Web,
        snapshot_tree_hash: None,
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
        .find_permission_request("req-1")
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
        .find_permission_request("req-1")
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
        forked.parent_session_id.as_deref(),
        Some(parent_session_id.as_str())
    );
    assert_eq!(forked.fork_from_node_seq, Some(6));
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

#[tokio::test]
async fn outbox_watermark_extraction_and_ack() {
    let mut fixture = Fixture::new(DbRole::Node).await;
    let events = fixture.seed_typical_stream().await;

    // Until ACKed, all events are in the outbox (batched per session)
    let outbox = fixture.db.extract_outbox().await.expect("outbox");
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
    let outbox = fixture.db.extract_outbox().await.expect("outbox");
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
    assert!(
        fixture
            .db
            .extract_outbox()
            .await
            .expect("outbox")
            .is_empty()
    );
    assert_eq!(fixture.db.unsynced_event_count().await.expect("count"), 0);

    // Range resend for ResyncRequest works regardless of the watermark
    let resend = fixture
        .db
        .extract_outbox_after(&fixture.session_id, 6)
        .await
        .expect("resync");
    assert_eq!(resend.len(), 3);
    assert_eq!(resend[0].node_seq, 6);
    // from_node_seq = 1 resends everything including SessionCreated
    let full = fixture
        .db
        .extract_outbox_after(&fixture.session_id, 1)
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
async fn fts_search_matches_text_and_excludes_binary_events() {
    let mut fixture = Fixture::new(DbRole::Node).await;
    let created = fixture.session_created();
    let user = fixture.event(UnifiedEventPayload::UserMessage {
        text: "認証エラーのデバッグをお願いします".to_owned(), // allow-japanese: CJK full-text search test
        attachments: vec![],
        client_source: ClientSource::Cli,
        snapshot_tree_hash: None,
    });
    let terminal = fixture.event(UnifiedEventPayload::TerminalOutput {
        terminal_id: "term-1".to_owned(),
        command: "cargo test".to_owned(),
        data_b64: "c2VjcmV0LXRva2VuLXZhbHVl".to_owned(), // "secret-token-value"
        exit_code: Some(0),
    });
    let message = fixture.event(UnifiedEventPayload::AgentMessage {
        message_id: "m-1".to_owned(),
        text: "Fixed the OpenCode2Driver connection".to_owned(),
        is_complete: true,
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
    let mut fixture = Fixture::new(DbRole::Node).await;
    let created = fixture.session_created();
    let message = fixture.event(UnifiedEventPayload::AgentMessage {
        message_id: "m-1".to_owned(),
        text: "Applied the migration".to_owned(),
        is_complete: true,
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
    let mut fixture = Fixture::new(DbRole::Node).await;
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
    let mut node_fixture = Fixture::with_session_id(DbRole::Node, session_id.clone()).await;
    let hub_fixture = Fixture::with_session_id(DbRole::Hub, session_id.clone()).await;

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
            .find_permission_request("req-1")
            .await
            .expect("node request"),
        hub_fixture
            .db
            .find_permission_request("req-1")
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
    let mut fixture = Fixture::new(DbRole::Node).await;
    let created = fixture.session_created();
    let plan = fixture.event(UnifiedEventPayload::PlanUpdate {
        entries: vec![PlanEntry {
            id: "1".to_owned(),
            title: "Add tests".to_owned(),
            status: PlanEntryStatus::Pending,
        }],
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
            project_id: Some(PROJECT_ID.to_owned()),
            ..SessionFilter::default()
        })
        .await
        .expect("filter by project");
    assert_eq!(by_project.len(), 1);

    let by_other_project = fixture
        .db
        .list_sessions(&SessionFilter {
            project_id: Some("github.com/other/repo".to_owned()),
            ..SessionFilter::default()
        })
        .await
        .expect("filter by other project");
    assert!(by_other_project.is_empty());

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
async fn registration_upserts_and_audit_logs() {
    let fixture = Fixture::new(DbRole::Hub).await;

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
            project_id: PROJECT_ID.to_owned(),
            node_id: NODE_ID.to_owned(),
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
async fn default_local_path_prefers_recent_binding_then_session_history() {
    let fixture = Fixture::new(DbRole::Hub).await;

    // Add a Worktree binding (almost the same time as the main repo → tie-break favors main)
    fixture
        .db
        .upsert_project_binding(&ProjectBindingRecord {
            project_id: PROJECT_ID.to_owned(),
            node_id: NODE_ID.to_owned(),
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
        .resolve_default_local_path(PROJECT_ID, NODE_ID)
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
        .resolve_default_local_path(PROJECT_ID, NODE_ID)
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
        .resolve_default_local_path(PROJECT_ID, NODE_ID)
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
        .resolve_default_local_path(PROJECT_ID, NODE_ID)
        .await
        .expect("resolve default");
    assert_eq!(path.as_deref(), Some("/home/nazo/src/flexagent-wt"));

    // A project without bindings falls back to the latest session's local_path
    // (assuming a session started before binding registration on session start existed)
    let legacy_project = "github.com/nazo6/legacy";
    let legacy_path = "/home/nazo/src/legacy";
    let legacy_event = SessionEventEnvelope {
        event_id: uuid_v7(),
        session_id: uuid_v7(),
        node_seq: 1,
        created_at: now_ms(),
        payload: UnifiedEventPayload::SessionCreated {
            node_id: NODE_ID.to_owned(),
            project_id: legacy_project.to_owned(),
            project_name: "legacy".to_owned(),
            local_path: legacy_path.to_owned(),
            git_branch: None,
            is_worktree: false,
            agent_id: "opencode2".to_owned(),
            parent_session_id: None,
            fork_from_node_seq: None,
            title: "New Session".to_owned(),
        },
    };
    fixture
        .db
        .append_events(std::slice::from_ref(&legacy_event))
        .await
        .expect("append legacy session");
    let path = fixture
        .db
        .resolve_default_local_path(legacy_project, NODE_ID)
        .await
        .expect("resolve default");
    assert_eq!(path.as_deref(), Some(legacy_path));

    // None when there are neither bindings nor sessions
    let path = fixture
        .db
        .resolve_default_local_path("github.com/nazo6/none", NODE_ID)
        .await
        .expect("resolve default");
    assert!(path.is_none());
}

#[tokio::test]
async fn file_backed_db_persists_across_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = fxg_db::node_db_path(dir.path());
    let session_id = uuid_v7();

    let db = Db::open(&path, DbRole::Node).await.expect("open file db");
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
        project_id: PROJECT_ID.to_owned(),
        name: "flexagent".to_owned(),
        canonical_git_url: None,
    })
    .await
    .expect("upsert project");
    db.append_event(&SessionEventEnvelope {
        event_id: uuid_v7(),
        session_id: session_id.clone(),
        node_seq: 1,
        created_at: now_ms(),
        payload: session_created_payload(),
    })
    .await
    .expect("append");
    db.close().await;

    let db = Db::open(&path, DbRole::Node).await.expect("reopen");
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
async fn append_next_event_allocates_gap_free_sequences() {
    let mut fixture = Fixture::new(DbRole::Node).await;
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
                snapshot_tree_hash: None,
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
    let mut fixture = Fixture::new(DbRole::Node).await;
    let created = fixture.session_created();
    let partial = fixture.event(UnifiedEventPayload::AgentMessage {
        message_id: "m1".to_owned(),
        text: "partial".to_owned(),
        is_complete: false,
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
    let mut fixture = Fixture::new(DbRole::Node).await;
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
    let mut fixture = Fixture::new(DbRole::Node).await;
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
            .find_permission_request("req-1")
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
    let outbox = fixture.db.extract_outbox().await.expect("outbox");
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].events.len(), 1);
    assert!(matches!(
        outbox[0].events[0].payload,
        UnifiedEventPayload::SessionDeleted {}
    ));
    assert_eq!(outbox[0].last_node_seq(), Some(9));

    // Events racing with deletion (e.g. turn completion during stop) are discarded
    let straggler = SessionEventEnvelope {
        event_id: uuid_v7(),
        session_id: fixture.session_id.clone(),
        node_seq: 10,
        created_at: now_ms(),
        payload: UnifiedEventPayload::UserMessage {
            text: "event received after deletion".to_owned(),
            attachments: vec![],
            client_source: ClientSource::Web,
            snapshot_tree_hash: None,
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
    let db = Db::open_in_memory(DbRole::Hub).await.expect("open db");
    let envelope = SessionEventEnvelope {
        event_id: uuid_v7(),
        session_id: uuid_v7(),
        node_seq: 5,
        created_at: now_ms(),
        payload: UnifiedEventPayload::SessionDeleted {},
    };
    let outcome = db.append_event(&envelope).await.expect("noop");
    assert!(!outcome.inserted);
}
