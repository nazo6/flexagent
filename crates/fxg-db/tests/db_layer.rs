//! `fxg-db` の統合テスト。
//!
//! Phase 1 完了条件の検証:
//! - マイグレーション適用 (単一スキーマ + FTS5 + トリガー)
//! - イベント適用の冪等性 (`INSERT OR IGNORE` + 同一トランザクション投影)
//! - 投影再構築とイベント適用結果の一致
//! - 水位ベース Outbox 抽出と ACK
//! - FTS5 全文検索 (トリガー経由・`searchable_text` 除外規則・LIKE フォールバック)
//! - Local Node (`node.db`) と Central Server (`server.db`) が同一レスポンス型を返すこと

use fxg_db::audit::actions;
use fxg_db::{
    AuditLogRecord, Db, DbError, DbRole, NodeRecord, ProjectBindingRecord, ProjectRecord,
    SessionFilter,
};
use fxg_protocol::common::{
    CommandInfo, ElicitationAction, ElicitationRequestStatus, ModeInfo, PermissionOption,
    PermissionRequestStatus, PlanEntry, SessionStatus, UsageCost,
};
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use fxg_protocol::util::{now_ms, uuid_v7};

const PROJECT_ID: &str = "github.com/nazo6/flexagent";
const NODE_ID: &str = "test-node";
const LOCAL_PATH: &str = "/home/nazo/src/flexagent";

/// テスト用の環境 (ノード・プロジェクト登録済みの DB とイベント採番ヘルパー)。
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

    /// 新しいイベント (node_seq を自動採番) を作る。
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

    /// 典型的なイベント列 (SessionCreated → タイトル → エージェント確定 → 状態 →
    /// ケイパビリティ → ユーザーメッセージ → 権限要求 → 権限解決) を流し込む。
    async fn seed_typical_stream(&mut self) -> Vec<SessionEventEnvelope> {
        let events = vec![
            self.session_created(),
            self.event(UnifiedEventPayload::SessionTitleChanged {
                title: "認証エラーの修正".to_owned(),
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
                    description: "コードレビュー".to_owned(),
                    input_hint: None,
                }],
                config_options: vec![],
            }),
            self.event(UnifiedEventPayload::UserMessage {
                text: "認証エラーを修正して".to_owned(),
                attachments: vec![],
                client_source: "cli".to_owned(),
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
                        kind: "allow_once".to_owned(),
                    },
                    PermissionOption {
                        option_id: "reject_once".to_owned(),
                        name: "Reject".to_owned(),
                        kind: "reject_once".to_owned(),
                    },
                ],
                details: serde_json::json!({ "command": "cargo test --workspace" }),
            }),
            self.event(UnifiedEventPayload::PermissionResolved {
                request_id: "req-1".to_owned(),
                selected_option_id: "allow_once".to_owned(),
                resolved_by: "cli".to_owned(),
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

/// `sessions.available_modes_json` を読み出してパースする。
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

    // 同一イベント (event_id も同一) の再送
    let second = fixture
        .db
        .append_event(&created)
        .await
        .expect("second append");
    assert!(!second.inserted, "duplicate event must be ignored");

    // 同一 (session_id, node_seq) で event_id だけ異なるイベント (再送の別採番)
    let mut duplicate_seq = created.clone();
    duplicate_seq.event_id = uuid_v7();
    let third = fixture
        .db
        .append_event(&duplicate_seq)
        .await
        .expect("third append");
    assert!(!third.inserted, "(session_id, node_seq) UNIQUE must hold");

    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 1);

    // 投影も重複適用されていない (タイトル・水位が変わらない)
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
    assert_eq!(summary.title, "認証エラーの修正");
    assert_eq!(summary.status, SessionStatus::Running);
    assert_eq!(summary.current_mode.as_deref(), Some("code"));
    assert_eq!(summary.last_node_seq, 8);

    // CapabilitiesUpdated の available_modes も投影カラムに反映される
    assert_eq!(
        available_modes(&fixture.db, &fixture.session_id).await,
        vec![ModeInfo {
            mode_id: "plan".to_owned(),
            name: "Plan".to_owned(),
            description: None,
        }]
    );

    // 承認は解決済みとして投影される
    let request = fixture
        .db
        .find_permission_request("req-1")
        .await
        .expect("find request")
        .expect("request exists");
    assert_eq!(request.status, PermissionRequestStatus::Approved);
    assert_eq!(request.resolved_by.as_deref(), Some("cli"));
    assert!(request.resolved_at.is_some());
    assert_eq!(request.options.len(), 2);
    assert_eq!(request.summary, "Run command: cargo test --workspace");
    assert_eq!(request.details["command"], "cargo test --workspace");
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
    // モードを含まない差分更新 (コマンドのみ) では available_modes を上書きしない
    let commands = fixture.event(UnifiedEventPayload::CapabilitiesUpdated {
        current_mode: None,
        available_modes: vec![],
        available_commands: vec![CommandInfo {
            name: "/review".to_owned(),
            description: "コードレビュー".to_owned(),
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

    // 全量再構築でも同じ投影が再現される
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
    // 2 回目の更新で完全に上書きされる (累積ではなく最新を保持する)
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

    // 全量再構築でも同じ投影が再現される
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
            kind: "reject_once".to_owned(),
        }],
        details: serde_json::json!({}),
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
        resolved_by: "android_push".to_owned(),
    });
    fixture.db.append_event(&resolved).await.expect("append");

    let request = fixture
        .db
        .find_permission_request("req-reject")
        .await
        .expect("find")
        .expect("exists");
    assert_eq!(request.status, PermissionRequestStatus::Rejected);
    assert_eq!(request.resolved_by.as_deref(), Some("android_push"));
}

#[tokio::test]
async fn elicitation_projection_tracks_pending_and_resolved() {
    let mut fixture = Fixture::new(DbRole::Node).await;
    let created = fixture.session_created();
    let request = fixture.event(UnifiedEventPayload::ElicitationRequest {
        elicitation_id: "elic-1".to_owned(),
        message: "どの戦略で進めますか?".to_owned(),
        mode: "form".to_owned(),
        requested_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "strategy": { "type": "string", "enum": ["a", "b"] }
            },
            "required": ["strategy"]
        }),
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
    assert_eq!(pending[0].requested_schema["type"], "object");

    let resolved = fixture.event(UnifiedEventPayload::ElicitationResolved {
        elicitation_id: "elic-1".to_owned(),
        action: ElicitationAction::Accept,
        content: serde_json::json!({ "strategy": "a" }),
        resolved_by: "web".to_owned(),
    });
    fixture.db.append_event(&resolved).await.expect("append");

    let entry = fixture
        .db
        .find_elicitation_request("elic-1")
        .await
        .expect("find")
        .expect("exists");
    assert_eq!(entry.status, ElicitationRequestStatus::Accepted);
    assert_eq!(entry.content["strategy"], "a");
    assert_eq!(entry.resolved_by.as_deref(), Some("web"));
    assert!(
        fixture
            .db
            .pending_elicitations()
            .await
            .expect("pending")
            .is_empty()
    );

    // 全量再構築でも同じ投影が再現される
    fixture.db.rebuild_projections().await.expect("rebuild");
    let rebuilt = fixture
        .db
        .find_elicitation_request("elic-1")
        .await
        .expect("find after rebuild")
        .expect("exists");
    assert_eq!(rebuilt.status, ElicitationRequestStatus::Accepted);
    assert_eq!(rebuilt.content["strategy"], "a");
}

#[tokio::test]
async fn rebuild_projections_matches_incremental_apply() {
    let mut fixture = Fixture::new(DbRole::Hub).await;
    fixture.seed_typical_stream().await;
    // 2つ目のセッション (Fork 相当) も追加して、複数セッションの再構築を検証する
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
        text: "Forkしたセッションです".to_owned(),
        attachments: vec![],
        client_source: "web".to_owned(),
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
        "全量再構築の結果はイベント適用結果と一致しなければならない"
    );
    assert_eq!(request_after, request_before);

    // Fork 元の情報が保持されている
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

    // 冪等: もう一度再構築しても変化しない
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

    // 未 ACK のうちは全イベントが Outbox に載る (セッション単位のバッチ)
    let outbox = fixture.db.extract_outbox().await.expect("outbox");
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].session_id, fixture.session_id);
    assert_eq!(outbox[0].events.len(), events.len());
    assert_eq!(outbox[0].last_node_seq(), Some(8));
    assert_eq!(fixture.db.unsynced_event_count().await.expect("count"), 8);
    assert_eq!(fixture.db.last_synced_at().await.expect("synced at"), None);

    // 3件目までの ACK で水位が進む
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

    // 巻き戻る ACK は無視される (MAX 更新)
    fixture
        .db
        .ack_watermark(&fixture.session_id, 1)
        .await
        .expect("ack older");
    assert_eq!(fixture.db.unsynced_event_count().await.expect("count"), 5);

    // 全 ACK で Outbox が空になる
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

    // ResyncRequest 用の範囲再送は水位に関係なく取得できる
    let resend = fixture
        .db
        .extract_outbox_after(&fixture.session_id, 6)
        .await
        .expect("resync");
    assert_eq!(resend.len(), 3);
    assert_eq!(resend[0].node_seq, 6);
    // from_node_seq = 1 は SessionCreated を含む全量再送
    let full = fixture
        .db
        .extract_outbox_after(&fixture.session_id, 1)
        .await
        .expect("full resync");
    assert_eq!(full.len(), 8);

    // 未知セッションへの ACK はエラー
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
        text: "認証エラーのデバッグをお願いします".to_owned(),
        attachments: vec![],
        client_source: "cli".to_owned(),
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
        text: "OpenCode2Driver の接続を修正しました".to_owned(),
        is_complete: true,
    });
    fixture
        .db
        .append_events(&[created, user, terminal, message])
        .await
        .expect("append");

    // FTS 経由 (トリガーで同期された検索インデックス) で日本語を検索できる
    let hits = fixture
        .db
        .search("認証エラー", 10)
        .await
        .expect("search japanese");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].event_type, "user_message");
    assert!(hits[0].snippet.contains("認証エラー"));
    assert!(hits[0].snippet.contains('['));

    // コード識別子の部分一致
    let hits = fixture
        .db
        .search("OpenCode2Driver", 10)
        .await
        .expect("search identifier");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].event_type, "agent_message");

    // TerminalOutput (バイナリ系) は FTS 対象外
    let hits = fixture
        .db
        .search("c2VjcmV0LXRva2VuLXZhbHVl", 10)
        .await
        .expect("search base64");
    assert!(hits.is_empty(), "terminal output must be excluded from FTS");

    // ヒットしない語
    let hits = fixture
        .db
        .search("存在しない検索語", 10)
        .await
        .expect("search missing");
    assert!(hits.is_empty());

    // 3文字未満は LIKE フォールバック
    let hits = fixture.db.search("認証", 10).await.expect("short query");
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("認証"));

    // 空クエリはエラー
    assert!(matches!(
        fixture.db.search("   ", 10).await.expect_err("empty"),
        DbError::EmptySearchQuery
    ));

    // LIKE メタ文字を含む検索語でもエラーにならない
    let hits = fixture.db.search("100%", 10).await.expect("like meta");
    assert!(hits.is_empty());
}

#[tokio::test]
async fn fts_index_stays_in_sync_on_regenerate_and_delete() {
    let mut fixture = Fixture::new(DbRole::Node).await;
    let created = fixture.session_created();
    let message = fixture.event(UnifiedEventPayload::AgentMessage {
        message_id: "m-1".to_owned(),
        text: "マイグレーションを適用しました".to_owned(),
        is_complete: true,
    });
    fixture
        .db
        .append_events(&[created, message])
        .await
        .expect("append");

    // searchable_text を破壊してから再生成 → 再度検索できる (UPDATE トリガー検証)
    sqlx::query("UPDATE session_events SET searchable_text = NULL")
        .execute(fixture.db.pool())
        .await
        .expect("corrupt searchable text");
    let hits = fixture
        .db
        .search("マイグレーション", 10)
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
        .search("マイグレーション", 10)
        .await
        .expect("search after regenerate");
    assert_eq!(hits.len(), 1);

    // FTS インデックスを content テーブルから全再構築しても同じ結果
    fixture.db.rebuild_fts_index().await.expect("rebuild fts");
    let hits = fixture
        .db
        .search("マイグレーション", 10)
        .await
        .expect("search after rebuild");
    assert_eq!(hits.len(), 1);

    // セッション削除 (CASCADE) でイベントと検索インデックスが同期する (DELETE トリガー検証)
    sqlx::query("DELETE FROM sessions WHERE session_id = ?")
        .bind(&fixture.session_id)
        .execute(fixture.db.pool())
        .await
        .expect("delete session");
    let hits = fixture
        .db
        .search("マイグレーション", 10)
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

    // イベントの payload が正しく復元される
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

    // ノード投影は登録時刻 (last_seen_at) を除いて一致する
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
    // プロジェクト投影も登録時刻を除いて一致する
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
            title: "テストを追加する".to_owned(),
            status: "pending".to_owned(),
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

    // プロジェクト / ノード / 状態によるフィルタ
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

    // ノード情報の更新
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

    // node_token ハッシュの発行と失効
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

    // オンライン状態の更新
    fixture
        .db
        .set_node_online(NODE_ID, false)
        .await
        .expect("offline");
    let nodes = fixture.db.list_nodes().await.expect("list nodes");
    assert!(!nodes[0].is_online);

    // バインドの更新 (ブランチ変更・パス消失)
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

    // 監査ログ
    let mut record = AuditLogRecord::new(actions::PERMISSION_RESOLVED, "192.168.1.10", "token:web");
    record.session_id = Some("s-1".to_owned());
    record.node_id = Some(NODE_ID.to_owned());
    record.client_user_agent = Some("Mozilla/5.0".to_owned());
    record.details = serde_json::json!({ "selected_option_id": "allow_once" });
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
    assert_eq!(logs[0].details["selected_option_id"], "allow_once");

    let limited = fixture.db.audit_logs(1).await.expect("audit logs");
    assert_eq!(limited.len(), 1);

    // 未知ノードへの操作はエラー
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

    // Worktree 紐付けを追加 (メインリポジトリとほぼ同時刻 → タイブレークでメイン優先)
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
    // タイブレーク検証のため両紐付けの最終使用日時を揃える
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
        "同時刻の場合はメインリポジトリを優先する"
    );

    // メインリポジトリの最終使用を古くすると直近使用の Worktree が既定になる
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
        "直近使用した Worktree を優先する"
    );

    // 実在しない紐付けは既定にしない (より古い実在する紐付けへフォールバック)
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
        "実在する紐付けへフォールバックする"
    );

    // 実在する紐付けが無い場合は最終使用が最も新しいものを返す (UI が警告表示)
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

    // 紐付けの無いプロジェクトは最新セッションの local_path にフォールバックする
    // (セッション開始時の紐付け登録を実装する前に実行されたセッションを想定)
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

    // 紐付けもセッションも無い場合は None
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
                text: "続けて".to_owned(),
                attachments: vec![],
                client_source: "cli".to_owned(),
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

    // 未知セッションへの採番はエラー
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

    // 永続化対象外のイベントは拒否される (キーストローク)
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
        DbError::NonPersistableEvent("terminal_input")
    ));

    // 拒否時は node_seq を消費しない (トランザクションがロールバックされる)
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
        text: "途中".to_owned(),
        is_complete: false,
    });

    let err = fixture
        .db
        .append_events(&[created.clone(), partial.clone()])
        .await
        .expect_err("incomplete message");
    assert!(matches!(err, DbError::NonPersistableEvent("agent_message")));

    // バッチ全体がロールバックされる (SessionCreated も入らない)
    assert!(
        fixture
            .db
            .get_session(&fixture.session_id)
            .await
            .expect("get")
            .is_none()
    );

    // 完成イベントなら追記できる
    let mut complete = partial;
    complete.payload = UnifiedEventPayload::AgentMessage {
        message_id: "m1".to_owned(),
        text: "完成".to_owned(),
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

    // イベントログは保持される (アーカイブは可逆な可視性フラグ)
    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 9);
    let summary = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(summary.archived_at, Some(archived.created_at));

    // 既定の一覧からは除外され、`include_archived` で取得できる
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

    // 全量再構築でもアーカイブ状態が維持される (イベント由来の投影)
    fixture.db.rebuild_projections().await.expect("rebuild");
    let rebuilt = fixture
        .db
        .get_session(&fixture.session_id)
        .await
        .expect("get rebuilt")
        .expect("exists");
    assert_eq!(rebuilt.archived_at, Some(archived.created_at));

    // 復元できる
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
    // 全イベントを同期済みにしてから削除する (Outbox には tombstone のみが載る)
    fixture
        .db
        .ack_watermark(&fixture.session_id, 8)
        .await
        .expect("ack");

    let deleted = fixture.event(UnifiedEventPayload::SessionDeleted {});
    fixture.db.append_event(&deleted).await.expect("delete");

    // 本文イベント・承認履歴はパージされ、tombstone のみ残る
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
        "削除済みセッションは取得できない (tombstone)"
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

    // 同期: tombstone のみが Outbox に載る
    let outbox = fixture.db.extract_outbox().await.expect("outbox");
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].events.len(), 1);
    assert!(matches!(
        outbox[0].events[0].payload,
        UnifiedEventPayload::SessionDeleted {}
    ));
    assert_eq!(outbox[0].last_node_seq(), Some(9));

    // 削除後に競合したイベント (停止処理中のターン完了など) は破棄される
    let straggler = SessionEventEnvelope {
        event_id: uuid_v7(),
        session_id: fixture.session_id.clone(),
        node_seq: 10,
        created_at: now_ms(),
        payload: UnifiedEventPayload::UserMessage {
            text: "削除後に届いたイベント".to_owned(),
            attachments: vec![],
            client_source: "web".to_owned(),
            snapshot_tree_hash: None,
        },
    };
    let outcome = fixture
        .db
        .append_event(&straggler)
        .await
        .expect("straggler");
    assert!(!outcome.inserted, "削除済みセッションへの追記は破棄される");
    assert_eq!(count_events(&fixture.db, &fixture.session_id).await, 1);

    // 全量再構築でも削除状態が維持される
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
    // Resync 応答などで未知セッションの tombstone が届いても FK 制約違反にしない
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
