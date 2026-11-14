use super::prompt::*;
use super::replay::*;
use super::*;
use crate::testutil::MockAgent;
use fxg_acp::AgentSessionInfo;
use fxg_acp::DriverKind;
use fxg_acp::agent::{
    AgentBackend, AgentCatalog, AgentDefinition, AgentExtension, CapabilitiesSnapshot,
    CapabilityOverlay, ConfigOptionPatch, ExtendedAcpDriver, LaunchPreset, SessionContext,
    StaticProvider,
};
use fxg_acp::registry::AcpRegistry;
use fxg_db::Db;
use fxg_db::queries::SessionFilter;
use fxg_protocol::SessionId;
use fxg_protocol::client_api::AgentCapabilitiesPreview;
use fxg_protocol::common::{
    ConfigOptionCategory, ConfigOptionInfo, ConfigOptionValue, EventType, InitialConfig, ModeInfo,
    PermissionOptionKind,
};
use fxg_protocol::paths::{Paths, Role};
use fxg_protocol::settings::{AgentSettings, CustomAgentConfig};

async fn setup() -> (SessionManager, MockAgent, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open_in_memory(Role::Node).await.expect("db");
    db.upsert_node(&fxg_db::NodeRecord::new(
        "test-node",
        "Test Node",
        "linux",
        "aarch64",
        "0.1.0",
    ))
    .await
    .expect("node");
    let bus = SessionEventBus::new(db);
    Paths::portable(dir.path(), Role::Node)
        .ensure_dirs()
        .expect("dirs");

    // Register a custom agent "mock" (resolved without an index)
    let mut custom = std::collections::BTreeMap::new();
    custom.insert(
        "mock".to_owned(),
        CustomAgentConfig {
            name: Some("Mock Agent".to_owned()),
            command: "mock".to_owned(),
            args: Vec::new(),
            env: Default::default(),
            env_secrets: Default::default(),
            extension: None,
            ..Default::default()
        },
    );
    let settings = AgentSettings {
        registry_url: None,
        registry_cache_ttl_secs: None,
        aliases: BTreeMap::new(),
        revision: 1,
        updated_at: 0,
    };
    let catalog = AgentCatalog::new(AcpRegistry::new(
        Paths::portable(dir.path(), Role::Node),
        &settings,
        custom,
    ));

    let mock = MockAgent::default();
    let manager = SessionManager::with_factory(bus, "test-node", catalog, mock.factory());
    (manager, mock, dir)
}

/// Waits until the session becomes Idle (waits for the driver's initial event processing).
async fn wait_idle(manager: &SessionManager, session_id: &str) {
    for _ in 0..100 {
        if manager
            .list_active()
            .iter()
            .any(|session| session.session_id == session_id && !session.busy)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("session did not become idle");
}

/// Waits until `busy = false` (waits for the turn to finish).
async fn wait_not_busy(manager: &SessionManager, session_id: &str) {
    for _ in 0..200 {
        let busy = manager
            .list_active()
            .iter()
            .find(|session| session.session_id == session_id)
            .map(|session| session.busy);
        match busy {
            Some(false) | None => return,
            _ => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
        }
    }
    panic!("session did not finish its turn");
}

/// Waits until a `UserMessage` with the given text is persisted and returns its `node_seq`.
async fn wait_user_message(manager: &SessionManager, session_id: &str, text: &str) -> u64 {
    for _ in 0..200 {
        let batch = manager
            .bus()
            .db()
            .session_events_after(session_id, 0, 100)
            .await
            .expect("events");
        if let Some(event) = batch.events.iter().find(|event| {
                matches!(&event.payload, UnifiedEventPayload::UserMessage { text: body, .. } if body == text)
            }) {
                return event.node_seq;
            }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("UserMessage was not recorded: {text}");
}

/// Waits until an event with the given `event_type` is persisted.
async fn wait_event_type(manager: &SessionManager, session_id: &str, event_type: EventType) {
    for _ in 0..200 {
        let batch = manager
            .bus()
            .db()
            .session_events_after(session_id, 0, 100)
            .await
            .expect("events");
        if batch
            .events
            .iter()
            .any(|event| event.payload.event_type() == event_type)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("event was not recorded: {event_type}");
}

/// Prepares a Git repository (`repo`) for tests and starts a session.
async fn setup_repo_session(
    manager: &SessionManager,
    dir: &tempfile::TempDir,
) -> (PathBuf, String) {
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    crate::testutil::init_test_repo(&repo).await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), &repo, "mock", &[], None)
        .await
        .expect("ensure");
    wait_idle(manager, &outcome.session_id).await;
    (repo, outcome.session_id)
}

#[tokio::test]
async fn ensure_session_records_created_event_and_start() {
    let (manager, _mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    assert!(!outcome.session_id.is_empty());

    // SessionCreated is persisted and the sessions projection exists
    let session = manager
        .bus()
        .db()
        .get_session(&outcome.session_id)
        .await
        .expect("query")
        .expect("session row");
    assert_eq!(session.agent_id, "mock");
}

#[tokio::test]
async fn probe_agent_capabilities_returns_options_and_caches() {
    let (manager, mock, dir) = setup().await;
    let preview = AgentCapabilitiesPreview {
        current_mode: Some("code".to_owned()),
        available_modes: vec![ModeInfo {
            mode_id: "code".to_owned(),
            name: "Code".to_owned(),
            description: None,
        }],
        config_options: vec![ConfigOptionInfo {
            key: "model".to_owned(),
            name: "Model".to_owned(),
            description: Some("Model used for the session".to_owned()),
            category: Some(ConfigOptionCategory::Model),
            current_value: ConfigOptionValue::Select("gpt-5".to_owned()),
            options: vec![
                ConfigOptionValue::Select("gpt-5".to_owned()),
                ConfigOptionValue::Select("gpt-4".to_owned()),
            ],
        }],
    };
    mock.set_start_capabilities(preview.clone());

    let probed = manager
        .probe_agent_capabilities("mock", dir.path(), &[], false)
        .await
        .expect("probe");
    assert_eq!(probed, preview);

    // The probe does not create an FXG session.
    assert!(manager.list_active().is_empty());
    let sessions = manager
        .bus()
        .db()
        .list_sessions(&SessionFilter::default())
        .await
        .expect("sessions");
    assert!(sessions.is_empty());

    // A second probe is served from the cache (no second driver start).
    mock.set_start_capabilities(AgentCapabilitiesPreview::default());
    let cached = manager
        .probe_agent_capabilities("mock", dir.path(), &[], false)
        .await
        .expect("cached probe");
    assert_eq!(cached, preview);
    assert_eq!(mock.starts().len(), 1);

    // A forced probe bypasses the cache and starts the driver again.
    let refreshed = manager
        .probe_agent_capabilities("mock", dir.path(), &[], true)
        .await
        .expect("forced probe");
    assert_eq!(refreshed, AgentCapabilitiesPreview::default());
    assert_eq!(mock.starts().len(), 2);

    // A model-specific probe is a different cache entry and is applied to the
    // temporary session so dependent options (effort, ...) can be discovered.
    let model_config = vec![InitialConfig {
        key: "model".to_owned(),
        value: ConfigOptionValue::Select("gpt-4".to_owned()),
    }];
    let _ = manager
        .probe_agent_capabilities("mock", dir.path(), &model_config, false)
        .await
        .expect("model probe");
    assert_eq!(mock.starts().len(), 3);
    let starts = mock.starts();
    assert_eq!(starts[2].initial_config, model_config);

    // The same model selection is served from the cache.
    let _ = manager
        .probe_agent_capabilities("mock", dir.path(), &model_config, false)
        .await
        .expect("cached model probe");
    assert_eq!(mock.starts().len(), 3);
}

struct ProbeExtension;

#[async_trait::async_trait]
impl AgentExtension for ProbeExtension {
    async fn capabilities(
        &self,
        _ctx: &SessionContext,
        _current: &CapabilitiesSnapshot,
    ) -> anyhow::Result<Option<CapabilityOverlay>> {
        Ok(Some(CapabilityOverlay {
            modes: Some(vec![ModeInfo {
                mode_id: "enriched".to_owned(),
                name: "Enriched".to_owned(),
                description: None,
            }]),
            current_mode: Some("enriched".to_owned()),
            config_options: vec![ConfigOptionPatch::Merge {
                key: "model".to_owned(),
                name: Some("Model (enriched)".to_owned()),
                options: None,
                current: None,
            }],
            add_commands: Vec::new(),
        }))
    }
}

#[tokio::test]
async fn probe_applies_extension_capability_overlay() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open_in_memory(Role::Node).await.expect("db");
    let bus = SessionEventBus::new(db);
    Paths::portable(dir.path(), Role::Node)
        .ensure_dirs()
        .expect("dirs");
    let settings = AgentSettings {
        registry_url: None,
        registry_cache_ttl_secs: None,
        aliases: BTreeMap::new(),
        revision: 1,
        updated_at: 0,
    };
    let provider = StaticProvider::new(
        AgentDefinition {
            id: "extended".to_owned(),
            name: "Extended".to_owned(),
            description: None,
            icon: None,
        },
        Some(LaunchPreset {
            program: "git",
            args: &["acp"],
        }),
    )
    .with_backend(AgentBackend::AcpExtended(Arc::new(ProbeExtension)));
    let catalog = AgentCatalog::with_providers(
        AcpRegistry::new(
            Paths::portable(dir.path(), Role::Node),
            &settings,
            BTreeMap::new(),
        ),
        vec![Arc::new(provider)],
    );

    let mock = MockAgent::default();
    let extended: Arc<dyn AgentDriver> = Arc::new(ExtendedAcpDriver::new(
        Arc::new(mock.clone()),
        Arc::new(ProbeExtension),
    ));
    let factory: DriverFactory = {
        let extended = Arc::clone(&extended);
        Arc::new(move |spec: &AgentLaunchSpec| {
            assert_eq!(spec.driver_kind, DriverKind::AcpExtended);
            Ok(Arc::clone(&extended))
        })
    };
    let manager = SessionManager::with_factory(bus, "test-node", catalog, factory);

    mock.set_start_capabilities(AgentCapabilitiesPreview {
        current_mode: Some("code".to_owned()),
        available_modes: vec![ModeInfo {
            mode_id: "code".to_owned(),
            name: "Code".to_owned(),
            description: None,
        }],
        config_options: vec![ConfigOptionInfo {
            key: "model".to_owned(),
            name: "Model".to_owned(),
            description: None,
            category: Some(ConfigOptionCategory::Model),
            current_value: ConfigOptionValue::Select("gpt-5".to_owned()),
            options: vec![ConfigOptionValue::Select("gpt-5".to_owned())],
        }],
    });

    let probed = manager
        .probe_agent_capabilities("extended", dir.path(), &[], true)
        .await
        .expect("probe");
    assert_eq!(probed.current_mode.as_deref(), Some("enriched"));
    assert_eq!(probed.available_modes[0].mode_id, "enriched");
    assert_eq!(probed.config_options[0].name, "Model (enriched)");
}

#[tokio::test]
async fn full_custom_driver_starts_through_the_catalog() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open_in_memory(Role::Node).await.expect("db");
    db.upsert_node(&fxg_db::NodeRecord::new(
        "test-node",
        "Test Node",
        "linux",
        "aarch64",
        "0.1.0",
    ))
    .await
    .expect("node");
    let bus = SessionEventBus::new(db);
    Paths::portable(dir.path(), Role::Node)
        .ensure_dirs()
        .expect("dirs");
    let settings = AgentSettings {
        registry_url: None,
        registry_cache_ttl_secs: None,
        aliases: BTreeMap::new(),
        revision: 1,
        updated_at: 0,
    };
    let mock = MockAgent::default();
    mock.set_start_capabilities(AgentCapabilitiesPreview::default());
    let provider = StaticProvider::new(
        AgentDefinition {
            id: "custom".to_owned(),
            name: "Custom".to_owned(),
            description: None,
            icon: None,
        },
        None,
    )
    .with_backend(AgentBackend::Custom(Arc::new(mock.clone())));
    let catalog = AgentCatalog::with_providers(
        AcpRegistry::new(
            Paths::portable(dir.path(), Role::Node),
            &settings,
            BTreeMap::new(),
        ),
        vec![Arc::new(provider)],
    );
    let manager = SessionManager::new(bus, "test-node", catalog);

    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "custom", &[], None)
        .await
        .expect("ensure");
    wait_idle(&manager, &outcome.session_id).await;

    let starts = mock.starts();
    assert_eq!(starts.len(), 1);
    assert_eq!(
        starts[0].cwd,
        dunce::canonicalize(dir.path()).expect("canonical cwd")
    );
}

#[tokio::test]
async fn full_custom_driver_probe_uses_the_catalog_driver() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open_in_memory(Role::Node).await.expect("db");
    let bus = SessionEventBus::new(db);
    Paths::portable(dir.path(), Role::Node)
        .ensure_dirs()
        .expect("dirs");
    let settings = AgentSettings {
        registry_url: None,
        registry_cache_ttl_secs: None,
        aliases: BTreeMap::new(),
        revision: 1,
        updated_at: 0,
    };
    let mock = MockAgent::default();
    let preview = AgentCapabilitiesPreview {
        current_mode: Some("code".to_owned()),
        available_modes: vec![ModeInfo {
            mode_id: "code".to_owned(),
            name: "Code".to_owned(),
            description: None,
        }],
        config_options: Vec::new(),
    };
    mock.set_start_capabilities(preview.clone());
    let provider = StaticProvider::new(
        AgentDefinition {
            id: "custom".to_owned(),
            name: "Custom".to_owned(),
            description: None,
            icon: None,
        },
        None,
    )
    .with_backend(AgentBackend::Custom(Arc::new(mock.clone())));
    let catalog = AgentCatalog::with_providers(
        AcpRegistry::new(
            Paths::portable(dir.path(), Role::Node),
            &settings,
            BTreeMap::new(),
        ),
        vec![Arc::new(provider)],
    );
    let manager = SessionManager::new(bus, "test-node", catalog);

    let probed = manager
        .probe_agent_capabilities("custom", dir.path(), &[], true)
        .await
        .expect("probe");
    assert_eq!(probed, preview);
    assert_eq!(mock.starts().len(), 1);
}

#[tokio::test]
async fn send_prompt_records_user_message_and_dispatches() {
    let (manager, mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    wait_idle(&manager, &outcome.session_id).await;

    manager
        .send_prompt(
            &CommandId::from("c2"),
            &outcome.session_id,
            &PromptInput::text("hello"),
            ClientSource::Cli,
        )
        .await
        .expect("prompt");

    // Sent to the driver and becomes busy
    for _ in 0..100 {
        if !mock.prompts().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(mock.prompts(), vec!["hello"]);
    let summary = manager
        .list_active()
        .into_iter()
        .find(|session| session.session_id == outcome.session_id)
        .expect("active");
    assert!(summary.busy);
}

#[tokio::test]
async fn prompt_attachments_are_resolved_and_validated() {
    let (manager, _mock, dir) = setup().await;
    let (repo, session_id) = setup_repo_session(&manager, &dir).await;
    let file = repo.join("note.md");
    std::fs::write(&file, "hello").expect("write attachment");

    let mut input = PromptInput::text("read this");
    input
        .attachments
        .push(fxg_protocol::common::AttachmentMeta {
            file_name: "note.md".to_owned(),
            mime_type: "text/markdown".to_owned(),
            size_bytes: 0,
            local_path: Some("note.md".to_owned()),
            data_b64: None,
        });
    manager
        .validate_prompt_attachments(&session_id, &mut input)
        .await
        .expect("inside workspace");
    let expected = fxg_pty::canonicalize(&file).expect("canonical");
    assert_eq!(
        input.attachments[0].local_path.as_deref(),
        Some(expected.to_string_lossy().as_ref())
    );
    assert_eq!(input.attachments[0].size_bytes, 5);

    let outside_file = dir.path().join("outside.txt");
    std::fs::write(&outside_file, "secret").expect("write outside attachment");
    let mut outside = PromptInput::text("read this");
    outside
        .attachments
        .push(fxg_protocol::common::AttachmentMeta {
            file_name: "outside.txt".to_owned(),
            mime_type: "text/plain".to_owned(),
            size_bytes: 0,
            local_path: Some(outside_file.to_string_lossy().into_owned()),
            data_b64: None,
        });
    let err = manager
        .validate_prompt_attachments(&session_id, &mut outside)
        .await
        .expect_err("outside workspace");
    assert!(matches!(err, NodeError::InvalidState(_)));
}

#[tokio::test]
async fn busy_prompt_is_queued_and_drained_on_idle() {
    let (manager, mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    wait_idle(&manager, &outcome.session_id).await;

    manager
        .send_prompt(
            &CommandId::from("c2"),
            &outcome.session_id,
            &PromptInput::text("first"),
            ClientSource::Cli,
        )
        .await
        .expect("first");
    // Enqueued while busy
    manager
        .send_prompt(
            &CommandId::from("c3"),
            &outcome.session_id,
            &PromptInput::text("second"),
            ClientSource::Web,
        )
        .await
        .expect("second queued");
    let summary = manager
        .list_active()
        .into_iter()
        .find(|session| session.session_id == outcome.session_id)
        .expect("active");
    assert_eq!(summary.queued_prompts, 1);
    assert_eq!(mock.prompts(), vec!["first"]);

    // Turn complete (idle) → the queue is dispatched automatically
    mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
        status: SessionStatus::Idle,
        error_message: None,
    }));
    for _ in 0..100 {
        if mock.prompts().len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(mock.prompts(), vec!["first", "second"]);
}

#[tokio::test]
async fn duplicate_command_id_is_rejected() {
    let (manager, _mock, dir) = setup().await;
    manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    let err = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect_err("duplicate");
    assert!(matches!(err, NodeError::CommandDuplicate(_)));
}

#[tokio::test]
async fn cancel_on_idle_session_is_safe_noop() {
    let (manager, _mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    wait_idle(&manager, &outcome.session_id).await;

    let res = manager
        .control(
            &CommandId::from("c2"),
            &outcome.session_id,
            &SessionControlAction::Cancel,
        )
        .await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn auto_approve_policy_responds_to_permission_requests() {
    let (manager, mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    wait_idle(&manager, &outcome.session_id).await;
    manager
        .control(
            &CommandId::from("c-policy"),
            &outcome.session_id,
            &SessionControlAction::SetPermissionPolicy {
                policy: PermissionPolicy::AutoAllowAll,
            },
        )
        .await
        .expect("set policy");

    mock.emit(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
        request_id: "req-auto".to_owned(),
        tool_name: "terminal".to_owned(),
        summary: "Run tests".to_owned(),
        options: vec![
            PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow once".to_owned(),
                kind: fxg_protocol::common::PermissionOptionKind::AllowOnce,
            },
            PermissionOption {
                option_id: "reject".to_owned(),
                name: "Reject".to_owned(),
                kind: fxg_protocol::common::PermissionOptionKind::RejectOnce,
            },
        ],
        details: serde_json::json!({}).into(),
        subagent_id: None,
    }));
    for _ in 0..100 {
        if !mock.permissions().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        mock.permissions(),
        vec![("req-auto".to_owned(), "allow_once".to_owned())]
    );
}

#[tokio::test]
async fn permission_policy_can_be_set_while_stopped() {
    let (manager, mock, dir) = setup().await;
    let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "stopped policy").await;

    manager
        .control(
            &CommandId::from("c-policy-stopped"),
            &session_id,
            &SessionControlAction::SetPermissionPolicy {
                policy: PermissionPolicy::AutoAllowAll,
            },
        )
        .await
        .expect("set policy while stopped");

    let row = manager
        .bus()
        .db()
        .get_session(&session_id)
        .await
        .expect("query")
        .expect("session row");
    assert_eq!(row.permission_policy, PermissionPolicy::AutoAllowAll);
}

#[tokio::test]
async fn resume_restores_permission_policy() {
    let (manager, mock, dir) = setup().await;
    let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "policy resume").await;

    manager
        .control(
            &CommandId::from("c-policy-stopped"),
            &session_id,
            &SessionControlAction::SetPermissionPolicy {
                policy: PermissionPolicy::AutoAllowAll,
            },
        )
        .await
        .expect("set policy while stopped");

    mock.set_resume_supported(true);
    manager
        .resume(ResumeParams {
            command_id: &CommandId::from("c-resume-policy"),
            session_id: &session_id,
            force_replay: false,
        })
        .await
        .expect("resume");
    wait_idle(&manager, &session_id).await;

    // The restored policy auto-approves the next permission request.
    mock.emit(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
        request_id: "req-restored".to_owned(),
        tool_name: "terminal".to_owned(),
        summary: "Run tests".to_owned(),
        options: vec![
            PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow once".to_owned(),
                kind: fxg_protocol::common::PermissionOptionKind::AllowOnce,
            },
            PermissionOption {
                option_id: "reject".to_owned(),
                name: "Reject".to_owned(),
                kind: fxg_protocol::common::PermissionOptionKind::RejectOnce,
            },
        ],
        details: serde_json::json!({}).into(),
        subagent_id: None,
    }));
    for _ in 0..100 {
        if !mock.permissions().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        mock.permissions(),
        vec![("req-restored".to_owned(), "allow_once".to_owned())]
    );
}

#[tokio::test]
async fn permission_resolution_is_idempotent() {
    let (manager, mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    wait_idle(&manager, &outcome.session_id).await;

    // Inject a permission request from the driver
    mock.emit(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
        request_id: "req-1".to_owned(),
        tool_name: "terminal".to_owned(),
        summary: "Run tests".to_owned(),
        options: vec![PermissionOption {
            option_id: "allow_once".to_owned(),
            name: "Allow once".to_owned(),
            kind: PermissionOptionKind::AllowOnce,
        }],
        details: serde_json::json!({}).into(),
        subagent_id: None,
    }));
    for _ in 0..100 {
        if !manager.pending_permissions().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(manager.pending_permissions().len(), 1);

    manager
        .respond_permission(
            &CommandId::from("c2"),
            &outcome.session_id,
            "req-1",
            "allow_once",
            ResolvedBy::Cli,
        )
        .await
        .expect("resolve");
    assert!(manager.pending_permissions().is_empty());

    // The second attempt is ALREADY_RESOLVED
    let err = manager
        .respond_permission(
            &CommandId::from("c3"),
            &outcome.session_id,
            "req-1",
            "allow_once",
            ResolvedBy::Cli,
        )
        .await
        .expect_err("already resolved");
    assert!(matches!(err, NodeError::AlreadyResolved(_)));
}

#[tokio::test]
async fn elicitation_resolution_is_idempotent() {
    let (manager, mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    wait_idle(&manager, &outcome.session_id).await;

    // Inject an elicitation (question) from the driver
    mock.emit(DriverEvent::Event(
        UnifiedEventPayload::ElicitationRequest {
            elicitation_id: "elic-1".to_owned(),
            message: "Which strategy should we go with?".to_owned(),
            mode: ElicitationMode::Form,
            requested_schema: serde_json::from_value(serde_json::json!({
                "type": "object",
                "properties": { "strategy": { "type": "string", "enum": ["a", "b"] } },
                "required": ["strategy"]
            }))
            .expect("schema"),
            tool_call_id: None,
            subagent_id: None,
        },
    ));
    for _ in 0..100 {
        if !manager.pending_elicitations().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(manager.pending_elicitations().len(), 1);
    assert_eq!(
        manager.pending_elicitations()[0].message,
        "Which strategy should we go with?"
    );

    manager
        .respond_elicitation(
            &CommandId::from("c2"),
            &outcome.session_id,
            "elic-1",
            ElicitationAction::Accept,
            Some(serde_json::from_value(serde_json::json!({ "strategy": "a" })).expect("content")),
            ResolvedBy::Cli,
        )
        .await
        .expect("resolve");
    assert!(manager.pending_elicitations().is_empty());
    let recorded = mock.elicitations();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "elic-1");
    assert_eq!(recorded[0].1, ElicitationAction::Accept);
    assert_eq!(
        recorded[0].2.as_ref().expect("content").get("strategy"),
        Some(&fxg_protocol::common::ElicitationValue::String(
            "a".to_owned()
        ))
    );

    // The second attempt is ALREADY_RESOLVED
    let err = manager
        .respond_elicitation(
            &CommandId::from("c3"),
            &outcome.session_id,
            "elic-1",
            ElicitationAction::Decline,
            None,
            ResolvedBy::Cli,
        )
        .await
        .expect_err("already resolved");
    assert!(matches!(err, NodeError::AlreadyResolved(_)));
}

#[tokio::test]
async fn driver_resolved_elicitation_clears_pending() {
    let (manager, mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    wait_idle(&manager, &outcome.session_id).await;

    // An elicitation issued by the driver appears in pending
    mock.emit(DriverEvent::Event(
        UnifiedEventPayload::ElicitationRequest {
            elicitation_id: "elic-2".to_owned(),
            message: "Do you want to continue?".to_owned(),
            mode: ElicitationMode::Form,
            requested_schema: serde_json::from_value(
                serde_json::json!({ "type": "object", "properties": {} }),
            )
            .expect("schema"),
            tool_call_id: None,
            subagent_id: None,
        },
    ));
    for _ in 0..100 {
        if !manager.pending_elicitations().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(manager.pending_elicitations().len(), 1);

    // Automatic resolution on the driver side (e.g. turn cancel) removes it from pending
    mock.emit(DriverEvent::Event(
        UnifiedEventPayload::ElicitationResolved {
            elicitation_id: "elic-2".to_owned(),
            action: ElicitationAction::Cancel,
            content: None,
            resolved_by: ResolvedBy::Unknown("system".to_owned()),
        },
    ));
    for _ in 0..100 {
        if manager.pending_elicitations().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(manager.pending_elicitations().is_empty());
}

#[tokio::test]
async fn import_session_records_and_resumes_an_existing_agent_session() {
    let (manager, mock, dir) = setup().await;
    mock.set_resume_supported(true);
    let session_id = manager
        .import_session(
            &CommandId::from("c-import"),
            "mock",
            "agent-session-1",
            dir.path(),
            Some("Imported"),
            false,
        )
        .await
        .expect("import");
    let starts = mock.starts();
    assert!(starts.iter().any(|start| {
        start
            .resume
            .as_ref()
            .and_then(|resume| resume.agent_session_id.as_deref())
            == Some("agent-session-1")
    }));
    let session = manager
        .bus()
        .db()
        .get_session(&session_id)
        .await
        .expect("query")
        .expect("session");
    assert_eq!(session.title, "Imported");
}

#[tokio::test]
async fn list_external_sessions_marks_already_imported_sessions() {
    let (manager, mock, dir) = setup().await;
    mock.set_resume_supported(true);
    mock.set_external_sessions(
        vec![
            AgentSessionInfo {
                session_id: "external-1".to_owned(),
                cwd: dir.path().to_path_buf(),
                title: Some("External one".to_owned()),
                updated_at: Some("2026-11-12T10:00:00Z".to_owned()),
            },
            AgentSessionInfo {
                session_id: "external-2".to_owned(),
                cwd: dir.path().to_path_buf(),
                title: None,
                updated_at: None,
            },
        ],
        true,
    );

    let response = manager
        .list_external_sessions(&["mock".to_owned()], true)
        .await
        .expect("list external");
    assert_eq!(response.sessions.len(), 2);
    assert!(response.agents.iter().any(|agent| {
        agent.agent_id == "mock" && agent.supports_list && agent.supports_history
    }));
    assert!(
        response
            .sessions
            .iter()
            .all(|session| session.imported_session_id.is_none())
    );

    let session_id = manager
        .import_session(
            &CommandId::from("c-import-external"),
            "mock",
            "external-1",
            dir.path(),
            Some("External one"),
            false,
        )
        .await
        .expect("import");

    // The `SessionAgentBound` projection is written by the event pump; wait for
    // it so discovery sees the imported session deterministically.
    for _ in 0..100 {
        if manager
            .bus()
            .db()
            .find_session_by_agent_session("mock", "external-1")
            .await
            .expect("lookup")
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let response = manager
        .list_external_sessions(&["mock".to_owned()], true)
        .await
        .expect("list external");
    let imported = response
        .sessions
        .iter()
        .find(|session| session.agent_session_id == "external-1")
        .expect("external-1");
    assert_eq!(
        imported.imported_session_id.as_ref().map(SessionId::as_str),
        Some(session_id.as_str())
    );
    assert!(
        response
            .sessions
            .iter()
            .find(|session| session.agent_session_id == "external-2")
            .expect("external-2")
            .imported_session_id
            .is_none()
    );
}

#[tokio::test]
async fn fork_uses_native_session_fork() {
    let (manager, mock, dir) = setup().await;
    let (repo, source_id) = setup_repo_session(&manager, &dir).await;

    // Native fork requires an agent-side session binding on the source.
    manager
        .bus()
        .record(
            &source_id,
            UnifiedEventPayload::SessionAgentBound {
                agent_session_id: "agent-source-1".to_owned(),
            },
        )
        .await
        .expect("bind agent session");
    manager
        .bus()
        .record(
            &source_id,
            UnifiedEventPayload::UserMessage {
                text: "parent question".to_owned(),
                attachments: vec![],
                client_source: ClientSource::Web,
            },
        )
        .await
        .expect("record parent question");
    manager
        .bus()
        .record(
            &source_id,
            UnifiedEventPayload::AgentMessage {
                message_id: "parent-message".to_owned(),
                text: "parent answer".to_owned(),
                is_complete: true,
                subagent_id: None,
            },
        )
        .await
        .expect("record parent answer");

    let fork = manager
        .fork(&CommandId::from("c3"), &source_id, None)
        .await
        .expect("fork");
    assert_ne!(fork.session_id, source_id);

    // The parent-child relationship is recorded.
    let child = manager
        .bus()
        .db()
        .get_session(&fork.session_id)
        .await
        .expect("query")
        .expect("child row");
    assert_eq!(
        child.parent_session_id.as_ref().map(SessionId::as_str),
        Some(source_id.as_str())
    );
    assert_eq!(child.agent_id, "mock");

    // The driver was asked to fork the source ACP session natively.
    let start = mock
        .starts()
        .into_iter()
        .find(|start| start.session_id == fork.session_id)
        .expect("fork start");
    assert_eq!(start.fork_from.as_deref(), Some("agent-source-1"));

    // The parent conversation is mirrored into the child's event log so the UI
    // shows the history the agent inherited.
    let batch = manager
        .bus()
        .db()
        .session_events_after(&fork.session_id, 0, 100)
        .await
        .expect("events");
    assert!(
        batch.events.iter().any(|event| matches!(
            &event.payload,
            UnifiedEventPayload::UserMessage { text, .. } if text == "parent question"
        )),
        "the fork must show the parent's user message"
    );
    assert!(
        batch.events.iter().any(|event| matches!(
            &event.payload,
            UnifiedEventPayload::AgentMessage { text, .. } if text == "parent answer"
        )),
        "the fork must show the parent's agent message"
    );
    // Session-scoped meta events stay with the source.
    assert!(
        !batch.events.iter().any(|event| matches!(
            &event.payload,
            UnifiedEventPayload::SessionAgentBound { agent_session_id }
                if agent_session_id == "agent-source-1"
        )),
        "the parent's meta events must not be copied"
    );
    // The copied events follow SessionCreated without gaps.
    let node_seqs: Vec<u64> = batch.events.iter().map(|event| event.node_seq).collect();
    assert_eq!(node_seqs, (1..=node_seqs.len() as u64).collect::<Vec<_>>());

    // The fork's working directory shares the same repository
    // (canonicalize before comparing because macOS resolves `/var` → `/private/var`)
    assert_eq!(
        PathBuf::from(&child.local_path)
            .canonicalize()
            .expect("canonicalize child"),
        repo.canonicalize().expect("canonicalize repo")
    );
}

#[tokio::test]
async fn fork_rejects_a_boundary_before_the_latest_event() {
    let (manager, mock, dir) = setup().await;
    let (repo, source_id) = setup_repo_session(&manager, &dir).await;
    manager
        .bus()
        .record(
            &source_id,
            UnifiedEventPayload::SessionAgentBound {
                agent_session_id: "agent-source-boundary".to_owned(),
            },
        )
        .await
        .expect("bind agent session");

    let child_id = fxg_protocol::util::uuid_v7();
    let err = manager
        .start_session(StartSessionParams {
            command_id: &CommandId::from("c-fork-boundary"),
            session_id: &child_id,
            local_path: &repo,
            agent_id: "mock",
            initial_prompt: None,
            initial_config: &[],
            mode: None,
            extra_args: None,
            fork_from_session_id: Some(&source_id),
            fork_up_to_node_seq: Some(1),
        })
        .await
        .expect_err("ACP-native forks cannot fork before the latest event");
    assert!(matches!(err, NodeError::ForkScopeUnsupported(_)));
    assert!(
        mock.starts()
            .iter()
            .all(|start| start.session_id != child_id),
        "the rejected fork must not start an agent session"
    );
}

#[tokio::test]
async fn fork_requires_agent_session_binding() {
    let (manager, _mock, dir) = setup().await;
    let (_repo, source_id) = setup_repo_session(&manager, &dir).await;

    let err = manager
        .fork(&CommandId::from("c3"), &source_id, None)
        .await
        .expect_err("native fork needs the source agent session id");
    assert!(matches!(err, NodeError::ForkUnsupported(_)));
}

#[tokio::test]
async fn start_session_with_fork_from_uses_native_fork() {
    let (manager, mock, dir) = setup().await;
    let (repo, source_id) = setup_repo_session(&manager, &dir).await;
    manager
        .bus()
        .record(
            &source_id,
            UnifiedEventPayload::SessionAgentBound {
                agent_session_id: "agent-source-2".to_owned(),
            },
        )
        .await
        .expect("bind agent session");
    manager
        .bus()
        .record(
            &source_id,
            UnifiedEventPayload::UserMessage {
                text: "forked context".to_owned(),
                attachments: vec![],
                client_source: ClientSource::Web,
            },
        )
        .await
        .expect("record parent question");

    let child_id = fxg_protocol::util::uuid_v7();
    let prompt = PromptInput::text("continue");
    manager
        .start_session(StartSessionParams {
            command_id: &CommandId::from("c-fork"),
            session_id: &child_id,
            local_path: &repo,
            agent_id: "ignored-agent",
            initial_prompt: Some(&prompt),
            initial_config: &[],
            mode: None,
            extra_args: None,
            fork_from_session_id: Some(&source_id),
            fork_up_to_node_seq: None,
        })
        .await
        .expect("start forked session");

    let child = manager
        .bus()
        .db()
        .get_session(&child_id)
        .await
        .expect("query")
        .expect("child row");
    assert_eq!(
        child.parent_session_id.as_ref().map(SessionId::as_str),
        Some(source_id.as_str())
    );
    assert_eq!(child.agent_id, "mock");

    let start = mock
        .starts()
        .into_iter()
        .find(|start| start.session_id == child_id)
        .expect("fork start");
    assert_eq!(start.fork_from.as_deref(), Some("agent-source-2"));
    assert!(
        mock.prompts().iter().any(|text| text == "continue"),
        "the initial prompt is sent after the fork"
    );

    // The parent conversation is mirrored into the child's event log.
    let batch = manager
        .bus()
        .db()
        .session_events_after(&child_id, 0, 100)
        .await
        .expect("events");
    assert!(
        batch.events.iter().any(|event| matches!(
            &event.payload,
            UnifiedEventPayload::UserMessage { text, .. } if text == "forked context"
        )),
        "start_session must copy the source conversation into the fork"
    );
}

#[tokio::test]
async fn driver_exit_records_stopped_status() {
    let (manager, mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    wait_idle(&manager, &outcome.session_id).await;

    // Driver (agent process) exit: when the event channel closes, the
    // pump records `Stopped` and removes it from the active list
    mock.close_events();
    for _ in 0..200 {
        let row = manager
            .bus()
            .db()
            .get_session(&outcome.session_id)
            .await
            .expect("query");
        if row.is_some_and(|session| session.status == SessionStatus::Stopped) {
            assert!(
                manager.list_active().is_empty(),
                "a stopped session is removed from the active list"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("driver exit did not record Stopped");
}

/// Waits until the session becomes `stopped` and is removed from the active list.
async fn wait_stopped(manager: &SessionManager, session_id: &str) {
    for _ in 0..200 {
        let row = manager
            .bus()
            .db()
            .get_session(session_id)
            .await
            .expect("query");
        if row.is_some_and(|session| session.status == SessionStatus::Stopped) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("session did not stop");
}

/// Prepares a stopped session with one conversation turn plus an agent-side ID.
///
/// Returns (repository path, session ID).
async fn setup_stopped_session(
    manager: &SessionManager,
    mock: &MockAgent,
    dir: &tempfile::TempDir,
    prompt: &str,
) -> (PathBuf, String) {
    let (repo, session_id) = setup_repo_session(manager, dir).await;
    manager
        .send_prompt(
            &CommandId::from("c2"),
            &session_id,
            &PromptInput::text(prompt),
            ClientSource::Cli,
        )
        .await
        .expect("prompt");
    wait_user_message(manager, &session_id, prompt).await;
    mock.emit(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
        agent_session_id: "agent-sess-1".to_owned(),
    }));
    wait_event_type(manager, &session_id, EventType::SessionAgentBound).await;
    mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
        status: SessionStatus::Idle,
        error_message: None,
    }));
    wait_not_busy(manager, &session_id).await;
    // Terminate the driver (agent process)
    mock.close_events();
    wait_stopped(manager, &session_id).await;
    (repo, session_id)
}

#[tokio::test]
async fn resume_restores_natively_without_replay() {
    let (manager, mock, dir) = setup().await;
    let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "native resume").await;

    mock.set_resume_supported(true);
    let outcome = manager
        .resume(ResumeParams {
            command_id: &CommandId::from("c-resume"),
            session_id: &session_id,
            force_replay: false,
        })
        .await
        .expect("resume");
    assert_eq!(outcome.session_id, session_id);
    assert!(outcome.context_restored);

    // After resume, status returns to idle and it rejoins the active list
    wait_idle(&manager, &session_id).await;
    let row = manager
        .bus()
        .db()
        .get_session(&session_id)
        .await
        .expect("query")
        .expect("session row");
    assert_eq!(row.status, SessionStatus::Idle);

    // The driver is asked to resume with the recorded agent-side ID
    let start = mock.starts().last().cloned().expect("start_session");
    let resume = start.resume.expect("resume request");
    assert_eq!(resume.agent_session_id.as_deref(), Some("agent-sess-1"));
    assert!(resume.allow_fresh);

    // No history replay is injected because of native restore
    let batch = manager
        .bus()
        .db()
        .session_events_after(&session_id, 0, 200)
        .await
        .expect("events");
    assert!(
            !batch.events.iter().any(|event| matches!(
                &event.payload,
                UnifiedEventPayload::UserMessage { client_source, .. } if client_source == &ClientSource::Resume
            )),
            "native resume must not inject replay context"
        );
}

#[tokio::test]
async fn resume_falls_back_to_replay_context() {
    let (manager, mock, dir) = setup().await;
    let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "fallback resume").await;

    // Driver without native restore support → continue the context via replay
    let outcome = manager
        .resume(ResumeParams {
            command_id: &CommandId::from("c-resume"),
            session_id: &session_id,
            force_replay: false,
        })
        .await
        .expect("resume");
    assert!(!outcome.context_restored);

    // The history replay is injected as a UserMessage with `client_source = "resume"`
    // and sent to the agent
    let batch = manager
        .bus()
        .db()
        .session_events_after(&session_id, 0, 200)
        .await
        .expect("events");
    let injected = batch
        .events
        .iter()
        .find_map(|event| match &event.payload {
            UnifiedEventPayload::UserMessage {
                text,
                client_source,
                ..
            } if client_source == &ClientSource::Resume => Some(text.clone()),
            _ => None,
        })
        .expect("injected resume context");
    assert!(injected.contains("fallback resume"), "history is included");
    assert!(
        mock.prompts().iter().any(|prompt| prompt == &injected),
        "the context is sent to the agent"
    );
    // Busy is cleared when the replay turn completes (Idle)
    mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
        status: SessionStatus::Idle,
        error_message: None,
    }));
    wait_not_busy(&manager, &session_id).await;
}

#[tokio::test]
async fn resume_force_replay_skips_agent_session_id() {
    let (manager, mock, dir) = setup().await;
    let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "force replay").await;

    // Even with native restore support, `--replay` does not pass the agent-side ID
    mock.set_resume_supported(true);
    let outcome = manager
        .resume(ResumeParams {
            command_id: &CommandId::from("c-resume"),
            session_id: &session_id,
            force_replay: true,
        })
        .await
        .expect("resume");
    assert!(!outcome.context_restored);
    assert_eq!(
        mock.starts()
            .last()
            .and_then(|start| start.resume.as_ref())
            .and_then(|resume| resume.agent_session_id.clone()),
        None
    );
}

#[tokio::test]
async fn resume_rejects_active_unknown_and_missing_workspace() {
    let (manager, mock, dir) = setup().await;

    // An unknown session yields InvalidSession
    let err = manager
        .resume(ResumeParams {
            command_id: &CommandId::from("u1"),
            session_id: "missing-session",
            force_replay: false,
        })
        .await
        .expect_err("unknown session");
    assert!(matches!(err, NodeError::InvalidSession(_)));

    // An active session yields InvalidState
    let (repo, session_id) = setup_repo_session(&manager, &dir).await;
    let err = manager
        .resume(ResumeParams {
            command_id: &CommandId::from("c2"),
            session_id: &session_id,
            force_replay: false,
        })
        .await
        .expect_err("active session");
    assert!(matches!(err, NodeError::InvalidState(_)));

    // A missing working directory (e.g. worktree removed) yields InvalidState
    mock.close_events();
    wait_stopped(&manager, &session_id).await;
    std::fs::remove_dir_all(&repo).expect("remove repo");
    let err = manager
        .resume(ResumeParams {
            command_id: &CommandId::from("c3"),
            session_id: &session_id,
            force_replay: false,
        })
        .await
        .expect_err("missing workspace");
    assert!(matches!(err, NodeError::InvalidState(_)));

    // Even on failure the exclusive guard (`resuming`) is released, so resume works after recovery
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    crate::testutil::init_test_repo(&repo).await;
    let outcome = manager
        .resume(ResumeParams {
            command_id: &CommandId::from("c4"),
            session_id: &session_id,
            force_replay: false,
        })
        .await
        .expect("resume after recovery");
    assert_eq!(outcome.session_id, session_id);
}

#[tokio::test]
async fn send_prompt_auto_resumes_stopped_session_natively() {
    let (manager, mock, dir) = setup().await;
    let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "first turn").await;

    // Agent with native restore support: sending without an explicit resume auto-resumes
    mock.set_resume_supported(true);
    manager
        .send_prompt(
            &CommandId::from("c-next"),
            &session_id,
            &PromptInput::text("auto resumed"),
            ClientSource::Web,
        )
        .await
        .expect("send prompt");

    // Native-only restore is requested with the recorded agent session ID
    let start = mock.starts().last().cloned().expect("start_session");
    let resume = start.resume.expect("resume request");
    assert_eq!(resume.agent_session_id.as_deref(), Some("agent-sess-1"));
    assert!(!resume.allow_fresh, "auto-resume is native-only");

    // When auto-resume completes, the state projection returns to idle and the prompt is sent
    wait_user_message(&manager, &session_id, "auto resumed").await;
    let row = manager
        .bus()
        .db()
        .get_session(&session_id)
        .await
        .expect("query")
        .expect("session row");
    assert_eq!(row.status, SessionStatus::Idle);
    assert!(mock.prompts().iter().any(|prompt| prompt == "auto resumed"));

    // No history replay is injected because of native restore
    let batch = manager
        .bus()
        .db()
        .session_events_after(&session_id, 0, 200)
        .await
        .expect("events");
    assert!(
            !batch.events.iter().any(|event| matches!(
                &event.payload,
                UnifiedEventPayload::UserMessage { client_source, .. } if client_source == &ClientSource::Resume
            )),
            "native auto resume must not inject replay context"
        );

    // Busy is cleared when the turn completes
    mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
        status: SessionStatus::Idle,
        error_message: None,
    }));
    wait_not_busy(&manager, &session_id).await;
}

#[tokio::test]
async fn send_prompt_requires_explicit_resume_when_native_unsupported() {
    let (manager, mock, dir) = setup().await;
    let (_repo, session_id) = setup_stopped_session(&manager, &mock, &dir, "first turn").await;

    // Native restore unsupported: the send fails with ResumeRequired and the
    // history replay is not implicitly run (the session stays stopped)
    let err = manager
        .send_prompt(
            &CommandId::from("c-next"),
            &session_id,
            &PromptInput::text("needs replay"),
            ClientSource::Web,
        )
        .await
        .expect_err("native resume is unavailable");
    assert!(matches!(err, NodeError::ResumeRequired { .. }));
    assert_eq!(
        err.error_code(),
        fxg_protocol::common::ErrorCode::ResumeRequired
    );

    let row = manager
        .bus()
        .db()
        .get_session(&session_id)
        .await
        .expect("query")
        .expect("session row");
    assert_eq!(row.status, SessionStatus::Stopped, "it remains stopped");
    assert!(manager.list_active().is_empty());
    assert!(
        !mock.prompts().iter().any(|prompt| prompt == "needs replay"),
        "the prompt is not sent"
    );

    // An explicit resume (history replay) can resume it
    let outcome = manager
        .resume(ResumeParams {
            command_id: &CommandId::from("c-resume"),
            session_id: &session_id,
            force_replay: false,
        })
        .await
        .expect("explicit resume");
    assert!(!outcome.context_restored);
}

#[tokio::test]
async fn session_status_transitions_on_driver_events() {
    let (manager, mock, dir) = setup().await;
    manager
        .ensure_session(&CommandId::from("c1"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");
    assert_eq!(manager.list_active().len(), 1);

    // StatusChanged from the driver is reflected in the session state
    mock.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
        status: SessionStatus::Stopped,
        error_message: None,
    }));
    for _ in 0..100 {
        let active = manager.list_active();
        if active
            .iter()
            .any(|session| session.status == SessionStatus::Stopped)
        {
            // The status is also reflected in the node.db sessions projection
            let session_id = active[0].session_id.clone();
            let row = manager
                .bus()
                .db()
                .get_session(&session_id)
                .await
                .expect("query")
                .expect("session row");
            assert_eq!(row.status, SessionStatus::Stopped);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("status did not become stopped");
}

#[test]
fn replay_context_keeps_most_recent_turns_when_truncated() {
    let old = "a".repeat(70_000);
    let context =
        assemble_replay_context(vec![old, "RECENT TURN".to_owned()], 10).expect("context");
    assert!(context.contains("RECENT TURN"));
    assert!(context.contains("older history omitted"));
    assert!(!context.contains(&"a".repeat(1_000)));
    assert!(context.chars().count() <= 64_000);
}

#[test]
fn derives_title_from_prompts() {
    assert_eq!(
        derive_title_from_prompt("Hello world"),
        Some("Hello world".to_owned())
    );
    assert_eq!(
        derive_title_from_prompt("# Task title\nSome description here"),
        Some("Task title".to_owned())
    );
    assert_eq!(
        derive_title_from_prompt("   \n\n> Blockquote prompt\nNext line"),
        Some("Blockquote prompt".to_owned())
    );
    let long = "あ".repeat(50); // allow-japanese: exercises multibyte character truncation
    let expected = format!("{}...", "あ".repeat(40)); // allow-japanese: same multibyte truncation
    assert_eq!(derive_title_from_prompt(&long), Some(expected));
    assert_eq!(derive_title_from_prompt("   \n\n  "), None);
}

#[tokio::test]
async fn start_session_uses_initial_prompt_for_title() {
    let (manager, _mock, dir) = setup().await;
    let session_id = fxg_protocol::util::uuid_v7();
    let initial_prompt = PromptInput::text("# First Turn\nDo something");
    manager
        .start_session(StartSessionParams {
            command_id: &CommandId::from("c_init"),
            session_id: &session_id,
            local_path: dir.path(),
            agent_id: "mock",
            initial_prompt: Some(&initial_prompt),
            initial_config: &[],
            mode: None,
            extra_args: None,
            fork_from_session_id: None,
            fork_up_to_node_seq: None,
        })
        .await
        .expect("start");

    let row = manager
        .bus()
        .db()
        .get_session(&session_id)
        .await
        .expect("query")
        .expect("session");
    assert_eq!(row.title, "First Turn");
}

#[tokio::test]
async fn first_prompt_updates_default_title() {
    let (manager, _mock, dir) = setup().await;
    let outcome = manager
        .ensure_session(&CommandId::from("c_ensure"), dir.path(), "mock", &[], None)
        .await
        .expect("ensure");

    // The initial title is the default
    let initial_row = manager
        .bus()
        .db()
        .get_session(&outcome.session_id)
        .await
        .expect("query")
        .expect("session");
    assert!(initial_row.title.contains('@'));

    // Send the first prompt
    manager
        .send_prompt(
            &CommandId::from("c_prompt1"),
            &outcome.session_id,
            &PromptInput::text("Implement login page"),
            ClientSource::Web,
        )
        .await
        .expect("prompt");

    // The title is updated
    let updated_row = manager
        .bus()
        .db()
        .get_session(&outcome.session_id)
        .await
        .expect("query")
        .expect("session");
    assert_eq!(updated_row.title, "Implement login page");

    // The title is not updated again on the second prompt (first time only)
    manager
        .send_prompt(
            &CommandId::from("c_prompt2"),
            &outcome.session_id,
            &PromptInput::text("Now add logout button"),
            ClientSource::Web,
        )
        .await
        .expect("prompt");

    let second_row = manager
        .bus()
        .db()
        .get_session(&outcome.session_id)
        .await
        .expect("query")
        .expect("session");
    assert_eq!(second_row.title, "Implement login page");
}

#[tokio::test]
async fn import_session_flushes_replayed_events_before_returning() {
    let (manager, mock, dir) = setup().await;
    mock.set_resume_supported(true);
    mock.set_start_replay_events(vec![
        UnifiedEventPayload::UserMessage {
            text: "earlier prompt".to_owned(),
            attachments: Vec::new(),
            client_source: ClientSource::Import,
        },
        UnifiedEventPayload::AgentMessage {
            message_id: "replay-1".to_owned(),
            text: "earlier answer".to_owned(),
            is_complete: true,
            subagent_id: None,
        },
    ]);

    let session_id = manager
        .import_session(
            &CommandId::from("c-import-flush"),
            "mock",
            "agent-ext-flush-1",
            dir.path(),
            None,
            true,
        )
        .await
        .expect("import succeeds");

    // Imported history must already be queryable in node.db without sleeping.
    let batch = manager
        .bus()
        .db()
        .session_events_after(&session_id, 0, 100)
        .await
        .expect("events");
    assert!(
        batch.events.iter().any(|event| matches!(
            &event.payload,
            UnifiedEventPayload::UserMessage { text, .. } if text == "earlier prompt"
        )),
        "expected replayed user message in node.db immediately after import, got: {:?}",
        batch.events
    );
    assert!(
        batch.events.iter().any(|event| matches!(
            &event.payload,
            UnifiedEventPayload::AgentMessage { text, .. } if text == "earlier answer"
        )),
        "expected replayed agent message in node.db immediately after import, got: {:?}",
        batch.events
    );
}
