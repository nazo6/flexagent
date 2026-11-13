use super::mapping::*;
use super::session::*;
use super::terminal::*;
use super::*;
use agent_client_protocol::v1::schema::{
    AuthMethodAgent, AuthMethodTerminal, Plan, PlanEntry as AcpPlanEntry, PlanEntryPriority,
};

fn text_chunk(text: &str, message_id: Option<&str>) -> SessionUpdate {
    // `ContentChunk` is non_exhaustive, so construct it via JSON
    let mut value = serde_json::json!({
        "content": { "type": "text", "text": text },
    });
    if let Some(id) = message_id {
        value["messageId"] = serde_json::Value::String(id.to_owned());
    }
    SessionUpdate::AgentMessageChunk(serde_json::from_value(value).expect("content chunk fixture"))
}

fn thought_chunk(text: &str, message_id: Option<&str>) -> SessionUpdate {
    let mut value = serde_json::json!({
        "content": { "type": "text", "text": text },
    });
    if let Some(id) = message_id {
        value["messageId"] = serde_json::Value::String(id.to_owned());
    }
    SessionUpdate::AgentThoughtChunk(serde_json::from_value(value).expect("content chunk fixture"))
}

fn user_chunk(text: &str, message_id: Option<&str>) -> SessionUpdate {
    let mut value = serde_json::json!({
        "content": { "type": "text", "text": text },
    });
    if let Some(id) = message_id {
        value["messageId"] = serde_json::Value::String(id.to_owned());
    }
    SessionUpdate::UserMessageChunk(serde_json::from_value(value).expect("content chunk fixture"))
}

/// Builds `AgentCapabilities` from a combination of capabilities.
fn agent_capabilities(load_session: bool, resume: bool) -> AgentCapabilities {
    // `AgentCapabilities` is non_exhaustive, so build it from default
    let mut capabilities = AgentCapabilities::default();
    capabilities.load_session = load_session;
    if resume {
        capabilities.session_capabilities.resume =
            Some(agent_client_protocol::v1::schema::SessionResumeCapabilities::default());
    }
    capabilities
}

fn resume_request(agent_session_id: Option<&str>, allow_fresh: bool) -> ResumeRequest {
    ResumeRequest {
        agent_session_id: agent_session_id.map(str::to_owned),
        allow_fresh,
        capture_history: false,
    }
}

fn elicitation_schema() -> ElicitationSchema {
    serde_json::from_value(serde_json::json!({
        "type": "object",
        "properties": {
            "strategy": { "type": "string", "enum": ["conservative", "balanced"] },
            "count": { "type": "integer" },
            "confirm": { "type": "boolean" },
            "tags": { "type": "array" }
        },
        "required": ["strategy"]
    }))
    .expect("schema")
}

fn elicitation_content(value: serde_json::Value) -> ElicitationContent {
    serde_json::from_value(value).expect("content")
}

#[test]
fn elicitation_content_validation_accepts_valid_values() {
    let schema = elicitation_schema();
    assert!(
        validate_elicitation_content(
            &schema,
            &elicitation_content(serde_json::json!({
                "strategy": "balanced",
                "count": 3,
                "confirm": true,
                "tags": ["a", "b"]
            }))
        )
        .is_ok()
    );
    // Do not validate schemas without properties
    assert!(
        validate_elicitation_content(
            &serde_json::from_value(serde_json::json!({ "type": "object" })).expect("empty schema"),
            &elicitation_content(serde_json::json!({ "anything": 1 }))
        )
        .is_ok()
    );
}

#[test]
fn elicitation_content_validation_rejects_invalid_values() {
    let schema = elicitation_schema();
    // Missing required property
    assert!(
        validate_elicitation_content(&schema, &elicitation_content(serde_json::json!({})))
            .expect_err("required")
            .contains("missing required property")
    );
    // Not in enum
    assert!(
        validate_elicitation_content(
            &schema,
            &elicitation_content(serde_json::json!({ "strategy": "fast" }))
        )
        .expect_err("enum")
        .contains("allowed values")
    );
    // Type mismatch
    assert!(
        validate_elicitation_content(
            &schema,
            &elicitation_content(serde_json::json!({
                "strategy": "balanced",
                "count": "3"
            }))
        )
        .expect_err("integer")
        .contains("must be an integer")
    );
    // Unknown property
    assert!(
        validate_elicitation_content(
            &schema,
            &elicitation_content(serde_json::json!({
                "strategy": "balanced",
                "extra": 1
            }))
        )
        .expect_err("unknown property")
        .contains("unexpected property")
    );
}

#[test]
fn elicitation_content_validation_checks_titled_enum() {
    let schema = serde_json::from_value(serde_json::json!({
        "type": "object",
        "properties": {
            "country": {
                "type": "string",
                "oneOf": [{ "const": "jp", "title": "Japan" }]
            }
        }
    }))
    .expect("schema");
    assert!(
        validate_elicitation_content(
            &schema,
            &elicitation_content(serde_json::json!({ "country": "jp" }))
        )
        .is_ok()
    );
    assert!(
        validate_elicitation_content(
            &schema,
            &elicitation_content(serde_json::json!({ "country": "us" }))
        )
        .expect_err("oneOf")
        .contains("allowed options")
    );
}

#[test]
fn plan_resume_prefers_session_resume_over_load() {
    // When both are supported, prefer `session/resume` (no history replay)
    let capabilities = agent_capabilities(true, true);
    let resume = resume_request(Some("ses_1"), true);
    match plan_resume(&capabilities, Some(&resume)).expect("plan") {
        ResumePlan::Resume(id) => assert_eq!(id, "ses_1"),
        ResumePlan::Load { .. } | ResumePlan::Fresh => {
            panic!("session/resume must be preferred")
        }
    }
}

#[test]
fn plan_resume_falls_back_to_load_then_fresh() {
    // Only load supported → session/load (replay discarded by the caller)
    let capabilities = agent_capabilities(true, false);
    let resume = resume_request(Some("ses_1"), true);
    match plan_resume(&capabilities, Some(&resume)).expect("plan") {
        ResumePlan::Load {
            agent_session_id,
            capture,
        } => {
            assert_eq!(agent_session_id, "ses_1");
            assert!(!capture);
        }
        ResumePlan::Resume(_) | ResumePlan::Fresh => panic!("session/load expected"),
    }

    // Neither supported + allow_fresh → fresh session (replay fallback)
    let capabilities = agent_capabilities(false, false);
    assert!(matches!(
        plan_resume(&capabilities, Some(&resume)).expect("plan"),
        ResumePlan::Fresh
    ));

    // Neither supported + no allow_fresh → native restore unavailable
    let resume = resume_request(Some("ses_1"), false);
    let err = plan_resume(&capabilities, Some(&resume)).expect_err("native-only must fail");
    assert!(err.downcast_ref::<NativeResumeUnavailable>().is_some());
}

#[test]
fn plan_resume_capture_prefers_load() {
    let capabilities = agent_capabilities(true, true);
    let mut resume = resume_request(Some("ses_1"), false);
    resume.capture_history = true;
    match plan_resume(&capabilities, Some(&resume)).expect("plan") {
        ResumePlan::Load {
            agent_session_id,
            capture,
        } => {
            assert_eq!(agent_session_id, "ses_1");
            assert!(capture);
        }
        ResumePlan::Resume(_) | ResumePlan::Fresh => {
            panic!("capture must use session/load even when resume exists")
        }
    }
}

#[test]
fn plan_resume_capture_requires_load() {
    let capabilities = agent_capabilities(false, true);
    let mut resume = resume_request(Some("ses_1"), false);
    resume.capture_history = true;
    let err = plan_resume(&capabilities, Some(&resume)).expect_err("load is required");
    assert!(err.downcast_ref::<HistoryCaptureUnavailable>().is_some());
}

#[test]
fn plan_resume_without_ids_is_fresh() {
    let capabilities = agent_capabilities(true, true);
    // No resume selection
    assert!(matches!(
        plan_resume(&capabilities, None).expect("plan"),
        ResumePlan::Fresh
    ));
    // No agent-side ID + allow_fresh (fresh session assuming history replay)
    let resume = resume_request(None, true);
    assert!(matches!(
        plan_resume(&capabilities, Some(&resume)).expect("plan"),
        ResumePlan::Fresh
    ));
    // No agent-side ID + no allow_fresh (automatic resume) means native restore is
    // unavailable (requires explicit resume via history replay)
    let resume = resume_request(None, false);
    let err = plan_resume(&capabilities, Some(&resume)).expect_err("native-only must fail");
    assert!(err.downcast_ref::<NativeResumeUnavailable>().is_some());
}

#[test]
fn accumulator_emits_completed_message_on_id_switch() {
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();
    let events = map_session_update(text_chunk("こん", Some("m1")), None, &mut acc, &mut caps); // allow-japanese: streaming chunk split across multibyte text
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], DriverEvent::Delta(_)));

    // Same ID: delta only
    let events = map_session_update(text_chunk("にちは", Some("m1")), None, &mut acc, &mut caps); // allow-japanese: streaming chunk split across multibyte text
    assert_eq!(events.len(), 1);

    // Different ID: completion event for the previous message + new delta
    let events = map_session_update(text_chunk("次", Some("m2")), None, &mut acc, &mut caps); // allow-japanese: multibyte streaming chunk fixture
    assert_eq!(events.len(), 2);
    match &events[0] {
        DriverEvent::Event(UnifiedEventPayload::AgentMessage {
            message_id,
            text,
            is_complete,
            ..
        }) => {
            assert_eq!(message_id, "m1");
            assert_eq!(text, "こんにちは"); // allow-japanese: expected accumulation of multibyte chunks
            assert!(is_complete);
        }
        other => panic!("unexpected event: {other:?}"),
    }

    // Flush the rest at turn end
    let finished = acc.finish_all();
    assert_eq!(finished.len(), 1);
    match &finished[0] {
        UnifiedEventPayload::AgentMessage {
            message_id, text, ..
        } => {
            assert_eq!(message_id, "m2");
            assert_eq!(text, "次"); // allow-japanese: expected multibyte chunk text
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[test]
fn import_replay_accumulates_user_and_agent_messages() {
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();
    let mut events = Vec::new();
    for update in [
        user_chunk("hello ", Some("u1")),
        user_chunk("world", Some("u1")),
        text_chunk("hi", Some("a1")),
        text_chunk(" there", Some("a1")),
    ] {
        events.extend(map_session_update_with_mode(
            update,
            None,
            &mut acc,
            &mut caps,
            UpdateMode::Import,
        ));
    }
    events.extend(acc.finish_all().into_iter().map(DriverEvent::Event));

    let payloads: Vec<_> = events
        .into_iter()
        .filter_map(|event| match event {
            DriverEvent::Event(payload) => Some(payload),
            DriverEvent::Delta(_) | DriverEvent::Failed { .. } => None,
        })
        .collect();

    assert_eq!(payloads.len(), 2);
    match &payloads[0] {
        UnifiedEventPayload::UserMessage {
            text,
            client_source,
            ..
        } => {
            assert_eq!(text, "hello world");
            assert_eq!(*client_source, fxg_protocol::common::ClientSource::Import);
        }
        other => panic!("unexpected payload: {other:?}"),
    }
    match &payloads[1] {
        UnifiedEventPayload::AgentMessage { text, .. } => assert_eq!(text, "hi there"),
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[test]
fn live_updates_ignore_user_message_chunks() {
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();
    let events = map_session_update(
        user_chunk("already recorded", None),
        None,
        &mut acc,
        &mut caps,
    );
    assert!(events.is_empty());
    assert!(acc.finish_all().is_empty());
}

#[test]
fn message_chunks_without_id_accumulate_into_single_message() {
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();
    map_session_update(text_chunk("a", None), None, &mut acc, &mut caps);
    map_session_update(text_chunk("b", None), None, &mut acc, &mut caps);
    let finished = acc.finish_all();
    assert_eq!(finished.len(), 1);
    match &finished[0] {
        UnifiedEventPayload::AgentMessage {
            message_id, text, ..
        } => {
            assert_eq!(message_id, "acp-message");
            assert_eq!(text, "ab");
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[test]
fn thought_is_completed_before_the_answer_that_follows_it() {
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();

    let events = map_session_update(
        thought_chunk("thinking", Some("m1")),
        None,
        &mut acc,
        &mut caps,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], DriverEvent::Delta(_)));

    // The first message chunk must finalize the reasoning stream so its
    // completion event is recorded before the answer.
    let events = map_session_update(text_chunk("answer", Some("m1")), None, &mut acc, &mut caps);
    assert_eq!(events.len(), 2);
    match &events[0] {
        DriverEvent::Event(UnifiedEventPayload::AgentThought {
            thought_id,
            text,
            is_complete,
            ..
        }) => {
            assert_eq!(thought_id, "m1");
            assert_eq!(text, "thinking");
            assert!(is_complete);
        }
        other => panic!("unexpected event: {other:?}"),
    }
    assert!(matches!(
        events[1],
        DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta { .. })
    ));

    let finished = acc.finish_all();
    assert_eq!(finished.len(), 1);
    assert!(matches!(
        finished[0],
        UnifiedEventPayload::AgentMessage { .. }
    ));
}

#[test]
fn finish_all_emits_thought_before_message() {
    // Defensive ordering for a state that the chunk APIs no longer produce
    // (both streams still open at turn end).
    let mut acc = StreamingAccumulator {
        messages: std::collections::BTreeMap::from([(
            None,
            ("m1".to_owned(), "answer".to_owned()),
        )]),
        thoughts: std::collections::BTreeMap::from([(
            None,
            ("t1".to_owned(), "thinking".to_owned()),
        )]),
        ..Default::default()
    };
    let finished = acc.finish_all();
    assert_eq!(finished.len(), 2);
    assert!(matches!(
        finished[0],
        UnifiedEventPayload::AgentThought { .. }
    ));
    assert!(matches!(
        finished[1],
        UnifiedEventPayload::AgentMessage { .. }
    ));
}

#[test]
fn thought_after_message_finalizes_the_message_first() {
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();

    map_session_update(text_chunk("answer", Some("m1")), None, &mut acc, &mut caps);
    let events = map_session_update(
        thought_chunk("thinking", Some("m2")),
        None,
        &mut acc,
        &mut caps,
    );
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        DriverEvent::Event(UnifiedEventPayload::AgentMessage { .. })
    ));
    assert!(matches!(
        events[1],
        DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta { .. })
    ));
}

#[test]
fn thought_and_message_are_completed_before_tool_call_and_plan() {
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();

    map_session_update(
        thought_chunk("thinking first", Some("t1")),
        None,
        &mut acc,
        &mut caps,
    );
    let events = map_session_update(
        SessionUpdate::ToolCall(ToolCall::new("tc-1", "Run command")),
        None,
        &mut acc,
        &mut caps,
    );
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        DriverEvent::Event(UnifiedEventPayload::AgentThought {
            text,
            is_complete: true,
            ..
        }) if text == "thinking first"
    ));
    assert!(matches!(
        &events[1],
        DriverEvent::Event(UnifiedEventPayload::ToolCall { tool_call_id, .. }) if tool_call_id == "tc-1"
    ));

    map_session_update(
        text_chunk("planning next", Some("m1")),
        None,
        &mut acc,
        &mut caps,
    );
    let events = map_session_update(
        SessionUpdate::Plan(Plan::new(vec![AcpPlanEntry::new(
            "Step 1",
            PlanEntryPriority::Medium,
            AcpPlanEntryStatus::Pending,
        )])),
        None,
        &mut acc,
        &mut caps,
    );
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        DriverEvent::Event(UnifiedEventPayload::AgentMessage {
            text,
            is_complete: true,
            ..
        }) if text == "planning next"
    ));
    assert!(matches!(
        &events[1],
        DriverEvent::Event(UnifiedEventPayload::PlanUpdate { .. })
    ));
}

#[test]
fn maps_tool_call_with_diff_content() {
    let tool_call = ToolCall::new("t-1", "Edit file")
        .kind(ToolKind::Edit)
        .status(ToolCallStatus::Completed)
        .content(vec![ToolCallContent::Diff(
            agent_client_protocol::v1::schema::Diff::new("src/lib.rs", "old\nnew\n"),
        )])
        .locations(vec![
            agent_client_protocol::v1::schema::ToolCallLocation::new("src/lib.rs"),
        ]);
    let payload = tool_call_event(&tool_call, None);
    match payload {
        UnifiedEventPayload::ToolCall {
            tool_call_id,
            kind,
            status,
            diff,
            locations,
            ..
        } => {
            assert_eq!(tool_call_id, "t-1");
            assert_eq!(kind, FxgToolCallKind::Edit);
            assert_eq!(status, FxgToolCallStatus::Completed);
            assert_eq!(locations, vec!["src/lib.rs"]);
            let diff = diff.expect("diff");
            assert_eq!(diff.path, "src/lib.rs");
            assert!(diff.additions >= 1);
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[test]
fn maps_plan_update() {
    let plan = Plan::new(vec![AcpPlanEntry::new(
        "Run tests",
        PlanEntryPriority::High,
        agent_client_protocol::v1::schema::PlanEntryStatus::InProgress,
    )]);
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();
    let events = map_session_update(SessionUpdate::Plan(plan), None, &mut acc, &mut caps);
    match &events[0] {
        DriverEvent::Event(UnifiedEventPayload::PlanUpdate { entries, .. }) => {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].title, "Run tests");
            assert_eq!(entries[0].status, FxgPlanEntryStatus::InProgress);
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn builds_unified_diff_with_counts() {
    let diff = build_file_diff("a.txt", "one\ntwo\n", "one\ntwo\nthree\n");
    assert_eq!(diff.additions, 1);
    assert_eq!(diff.deletions, 0);
    assert!(diff.unified_diff.contains("+three"));
    assert_eq!(diff.old_text.as_deref(), Some("one\ntwo\n"));
    assert_eq!(diff.new_text.as_deref(), Some("one\ntwo\nthree\n"));
}

#[test]
fn resolves_relative_paths_against_cwd() {
    let cwd = PathBuf::from("/tmp/session");
    assert_eq!(
        resolve_path(&cwd, Path::new("src/lib.rs")),
        PathBuf::from("/tmp/session/src/lib.rs")
    );
    assert_eq!(
        resolve_path(&cwd, Path::new("/etc/hosts")),
        PathBuf::from("/etc/hosts")
    );
}

#[test]
fn base64_encodes_terminal_output() {
    assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
}

#[test]
fn maps_session_info_update() {
    use agent_client_protocol::v1::schema::SessionInfoUpdate;

    let update = SessionInfoUpdate::new().title("Refactor UI layout".to_owned());
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();
    let events = map_session_update(
        SessionUpdate::SessionInfoUpdate(update),
        None,
        &mut acc,
        &mut caps,
    );
    assert_eq!(events.len(), 1);
    match &events[0] {
        DriverEvent::Event(UnifiedEventPayload::SessionTitleChanged { title }) => {
            assert_eq!(title, "Refactor UI layout");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn capabilities_partial_updates_preserve_existing_state() {
    use agent_client_protocol::v1::schema::{
        AvailableCommand, AvailableCommandsUpdate, CurrentModeUpdate,
    };

    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState {
        current_mode: Some("ask".to_owned()),
        available_modes: vec![ModeInfo {
            mode_id: "ask".to_owned(),
            name: "Ask".to_owned(),
            description: None,
        }],
        available_commands: Vec::new(),
        config_options: Vec::new(),
        fork_scope: None,
    };

    // Existing current_mode / available_modes are preserved when a command list update arrives
    let cmd_update =
        AvailableCommandsUpdate::new(vec![AvailableCommand::new("review", "Review code")]);
    let events = map_session_update(
        SessionUpdate::AvailableCommandsUpdate(cmd_update),
        None,
        &mut acc,
        &mut caps,
    );
    assert_eq!(events.len(), 1);
    match &events[0] {
        DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            available_commands,
            ..
        }) => {
            assert_eq!(current_mode.as_deref(), Some("ask"));
            assert_eq!(available_modes.len(), 1);
            assert_eq!(available_commands.len(), 1);
            assert_eq!(available_commands[0].name, "review");
        }
        other => panic!("unexpected event: {other:?}"),
    }

    // Previously added available_commands are preserved when a mode change arrives
    let mode_update = CurrentModeUpdate::new("code");
    let events = map_session_update(
        SessionUpdate::CurrentModeUpdate(mode_update),
        None,
        &mut acc,
        &mut caps,
    );
    assert_eq!(events.len(), 1);
    match &events[0] {
        DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            available_commands,
            ..
        }) => {
            assert_eq!(current_mode.as_deref(), Some("code"));
            assert_eq!(available_modes.len(), 1);
            assert_eq!(available_commands.len(), 1);
            assert_eq!(available_commands[0].name, "review");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn maps_grouped_select_options() {
    use agent_client_protocol::v1::schema::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelect,
        SessionConfigSelectGroup, SessionConfigSelectOption,
    };

    let option = SessionConfigOption::new(
        "model",
        "Model",
        SessionConfigKind::Select(SessionConfigSelect::new(
            "gpt-4",
            vec![
                SessionConfigSelectGroup::new(
                    "openai",
                    "OpenAI",
                    vec![
                        SessionConfigSelectOption::new("gpt-4", "GPT-4"),
                        SessionConfigSelectOption::new("gpt-3.5", "GPT-3.5"),
                    ],
                ),
                SessionConfigSelectGroup::new(
                    "anthropic",
                    "Anthropic",
                    vec![SessionConfigSelectOption::new("claude-3", "Claude 3")],
                ),
            ],
        )),
    )
    .description("Model used for the session")
    .category(SessionConfigOptionCategory::Model);

    let mapped = map_config_option(&option);
    assert_eq!(mapped.key, "model");
    assert_eq!(mapped.name, "Model");
    assert_eq!(
        mapped.description.as_deref(),
        Some("Model used for the session")
    );
    assert_eq!(
        mapped.category,
        Some(fxg_protocol::common::ConfigOptionCategory::Model)
    );
    assert_eq!(
        mapped.current_value,
        ConfigOptionValue::Select("gpt-4".to_owned())
    );
    assert_eq!(
        mapped.options,
        vec![
            ConfigOptionValue::Select("gpt-4".to_owned()),
            ConfigOptionValue::Select("gpt-3.5".to_owned()),
            ConfigOptionValue::Select("claude-3".to_owned()),
        ]
    );
}

#[test]
fn parses_bare_config_value_ids() {
    let value = parse_config_value(ConfigOptionValue::Select(
        "opencode-go/deepseek-v4.1-flash".to_owned(),
    ))
    .expect("bare string config value");
    assert_eq!(
        value.as_value_id().map(ToString::to_string).as_deref(),
        Some("opencode-go/deepseek-v4.1-flash")
    );
}

#[test]
fn parses_boolean_and_tagged_config_values() {
    let bool_value = parse_config_value(ConfigOptionValue::Boolean(true)).expect("bare boolean");
    assert_eq!(bool_value.as_bool(), Some(true));

    let tagged = parse_config_value(ConfigOptionValue::Unknown(
        serde_json::json!({
            "type": "boolean",
            "value": false
        })
        .into(),
    ))
    .expect("tagged boolean");
    assert_eq!(tagged.as_bool(), Some(false));

    let wire_id = parse_config_value(ConfigOptionValue::Unknown(
        serde_json::json!({ "value": "plan" }).into(),
    ))
    .expect("wire id");
    assert_eq!(
        wire_id.as_value_id().map(ToString::to_string).as_deref(),
        Some("plan")
    );
}

#[test]
fn rejects_unknown_config_values() {
    assert!(
        parse_config_value(ConfigOptionValue::Unknown(
            serde_json::json!({ "unexpected": true }).into()
        ))
        .is_err()
    );
}

#[test]
fn maps_usage_update_to_event() {
    use agent_client_protocol::v1::schema::{Cost, UsageUpdate};

    let update =
        SessionUpdate::UsageUpdate(UsageUpdate::new(53_000, 200_000).cost(Cost::new(0.045, "USD")));
    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();
    let events = map_session_update(update, None, &mut acc, &mut caps);
    assert_eq!(events.len(), 1);
    match &events[0] {
        DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
            used_tokens,
            context_size,
            cost,
            ..
        }) => {
            assert_eq!(*used_tokens, 53_000);
            assert_eq!(*context_size, 200_000);
            let cost = cost.as_ref().expect("cost");
            assert_eq!(cost.currency, "USD");
            assert!((cost.amount - 0.045).abs() < f64::EPSILON);
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn stop_reason_event_only_for_abnormal_reasons() {
    // Normal completion and user cancellation are not turned into events
    assert!(turn_ended_event(StopReason::EndTurn).is_none());
    assert!(turn_ended_event(StopReason::Cancelled).is_none());

    for (reason, expected) in [
        (StopReason::MaxTokens, TurnStopReason::MaxTokens),
        (StopReason::MaxTurnRequests, TurnStopReason::MaxTurnRequests),
        (StopReason::Refusal, TurnStopReason::Refusal),
    ] {
        match turn_ended_event(reason) {
            Some(UnifiedEventPayload::TurnEnded { reason }) => {
                assert_eq!(reason, expected);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }
}

#[test]
fn maps_agent_and_terminal_auth_methods() {
    let agent = AuthMethod::Agent(
        AuthMethodAgent::new("agent-login", "Agent login").description("browser login"),
    );
    let mapped = map_auth_method(&agent);
    assert_eq!(mapped.id, "agent-login");
    assert_eq!(mapped.kind, AgentAuthMethodKind::Agent);
    assert_eq!(mapped.description.as_deref(), Some("browser login"));
    assert!(mapped.args.is_empty());

    let terminal = AuthMethod::Terminal(
        AuthMethodTerminal::new("terminal-login", "Terminal login")
            .args(vec!["--login".to_owned()]),
    );
    let mapped = map_auth_method(&terminal);
    assert_eq!(mapped.id, "terminal-login");
    assert_eq!(mapped.kind, AgentAuthMethodKind::Terminal);
    assert_eq!(mapped.args, vec!["--login"]);
}

#[test]
fn client_capabilities_advertise_terminal_auth() {
    let capabilities =
        client_capabilities(&crate::agent::extension::AcpExtensionOptions::default());
    assert!(capabilities.terminal);
    assert!(capabilities.auth.terminal);
}

#[test]
fn prompt_blocks_sends_images_when_supported() {
    let input = PromptInput {
        text: "look".to_owned(),
        attachments: vec![fxg_protocol::common::AttachmentMeta {
            file_name: "shot.png".to_owned(),
            mime_type: "image/png".to_owned(),
            size_bytes: 3,
            local_path: None,
            data_b64: Some("YWJj".to_owned()),
        }],
    };
    let capabilities = PromptCapabilities::new().image(true).embedded_context(true);
    let blocks = prompt_blocks(input, &capabilities).expect("blocks");
    assert!(matches!(blocks[0], ContentBlock::Text(_)));
    assert!(matches!(blocks[1], ContentBlock::Image(_)));
}

#[test]
fn prompt_blocks_falls_back_to_text_without_image_support() {
    let input = PromptInput {
        text: String::new(),
        attachments: vec![fxg_protocol::common::AttachmentMeta {
            file_name: "shot.png".to_owned(),
            mime_type: "image/png".to_owned(),
            size_bytes: 3,
            local_path: None,
            data_b64: Some("YWJj".to_owned()),
        }],
    };
    let capabilities = PromptCapabilities::default();
    let blocks = prompt_blocks(input, &capabilities).expect("blocks");
    assert!(matches!(blocks[0], ContentBlock::Text(_)));
}

#[cfg(unix)]
#[test]
fn prompt_blocks_links_non_image_files() {
    let input = PromptInput {
        text: "see this".to_owned(),
        attachments: vec![fxg_protocol::common::AttachmentMeta {
            file_name: "notes.md".to_owned(),
            mime_type: "text/markdown".to_owned(),
            size_bytes: 10,
            local_path: Some("/tmp/notes.md".to_owned()),
            data_b64: None,
        }],
    };
    let blocks = prompt_blocks(input, &PromptCapabilities::default()).expect("blocks");
    match &blocks[1] {
        ContentBlock::ResourceLink(link) => {
            assert_eq!(link.name, "notes.md");
            assert_eq!(link.uri, "file:///tmp/notes.md");
        }
        other => panic!("unexpected block: {other:?}"),
    }
}

#[test]
fn tool_call_contents_extracts_text_output() {
    let contents = vec![ToolCallContent::Content(
        agent_client_protocol::v1::schema::Content::new(ContentBlock::Text(TextContent::new(
            "test output",
        ))),
    )];
    let (diff, locations, texts) = tool_call_contents(&contents);
    assert!(diff.is_none());
    assert!(locations.is_empty());
    assert_eq!(texts, vec!["test output"]);
}

#[test]
fn stderr_registry_is_bounded_and_per_agent() {
    let registry = StderrRegistry::default();
    for index in 0..(StderrRegistry::MAX_LINES + 5) {
        registry.push("agy", &format!("line {index}"));
    }
    registry.push("opencode", "other");
    let agy = registry.tail("agy");
    assert_eq!(agy.len(), StderrRegistry::MAX_LINES);
    assert_eq!(agy.last().map(String::as_str), Some("line 204"));
    assert_eq!(registry.tail("opencode"), vec!["other"]);
    assert_eq!(registry.snapshot().len(), 2);
}

/// Builds a `category: mode` select option the way OpenCode reports build/plan.
fn mode_config_option(current: &str, modes: &[(&str, &str)]) -> SessionConfigOption {
    use agent_client_protocol::v1::schema::SessionConfigSelectOption;

    SessionConfigOption::select(
        "mode",
        "Session Mode",
        current.to_owned(),
        modes
            .iter()
            .map(|(id, name)| SessionConfigSelectOption::new((*id).to_owned(), (*name).to_owned()))
            .collect::<Vec<_>>(),
    )
    .category(agent_client_protocol::v1::schema::SessionConfigOptionCategory::Mode)
}

#[test]
fn session_capabilities_derives_modes_from_config_options() {
    let options = vec![
        SessionConfigOption::select(
            "model",
            "Model",
            "m1",
            vec![
                agent_client_protocol::v1::schema::SessionConfigSelectOption::new("m1", "Model 1"),
            ],
        ),
        mode_config_option("build", &[("build", "Build"), ("plan", "Plan")]),
    ];
    let (modes, current_mode, config_options) = session_capabilities(None, Some(&options));

    assert_eq!(current_mode.as_deref(), Some("build"));
    assert_eq!(
        modes,
        vec![
            ModeInfo {
                mode_id: "build".to_owned(),
                name: "Build".to_owned(),
                description: None,
            },
            ModeInfo {
                mode_id: "plan".to_owned(),
                name: "Plan".to_owned(),
                description: None,
            },
        ]
    );
    assert_eq!(config_options.len(), 2);
}

#[test]
fn session_capabilities_prefers_config_option_over_legacy_modes() {
    // ACP: clients that support config options SHOULD ignore `modes` when both
    // are present.
    let legacy_modes: SessionModeState = serde_json::from_value(serde_json::json!({
        "currentModeId": "ask",
        "availableModes": [{ "id": "ask", "name": "Ask" }],
    }))
    .expect("session mode state fixture");
    let options = vec![mode_config_option(
        "plan",
        &[("build", "Build"), ("plan", "Plan")],
    )];

    let (modes, current_mode, _) = session_capabilities(Some(&legacy_modes), Some(&options));

    assert_eq!(current_mode.as_deref(), Some("plan"));
    assert_eq!(
        modes
            .iter()
            .map(|mode| mode.mode_id.as_str())
            .collect::<Vec<_>>(),
        vec!["build", "plan"]
    );
}

#[test]
fn config_option_update_refreshes_mode_projection() {
    use agent_client_protocol::v1::schema::ConfigOptionUpdate;

    let mut caps = SessionCapabilitiesState {
        current_mode: Some("build".to_owned()),
        available_modes: vec![
            ModeInfo {
                mode_id: "build".to_owned(),
                name: "Build".to_owned(),
                description: Some("The default agent".to_owned()),
            },
            ModeInfo {
                mode_id: "plan".to_owned(),
                name: "Plan".to_owned(),
                description: None,
            },
        ],
        ..Default::default()
    };
    let mut acc = StreamingAccumulator::default();
    let update = ConfigOptionUpdate::new(vec![mode_config_option(
        "plan",
        &[("build", "Build"), ("plan", "Plan")],
    )]);

    let events = map_session_update(
        SessionUpdate::ConfigOptionUpdate(update),
        None,
        &mut acc,
        &mut caps,
    );

    assert_eq!(events.len(), 1);
    match &events[0] {
        DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            ..
        }) => {
            assert_eq!(current_mode.as_deref(), Some("plan"));
            // The update carries the agent's labels and keeps both modes
            assert_eq!(available_modes.len(), 2);
            assert_eq!(available_modes[0].name, "Build");
            assert_eq!(available_modes[1].name, "Plan");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn child_updates_are_tagged_and_do_not_touch_parent_capabilities() {
    use agent_client_protocol::v1::schema::{AvailableCommand, AvailableCommandsUpdate};

    let mut acc = StreamingAccumulator::default();
    let mut caps = SessionCapabilitiesState::default();

    let events = map_session_update(
        text_chunk("child output", Some("child-m1")),
        Some("child-1"),
        &mut acc,
        &mut caps,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta { subagent_id: Some(child), .. })
            if child == "child-1"
    ));

    let finished = acc.finish_all();
    assert!(matches!(
        &finished[0],
        UnifiedEventPayload::AgentMessage { subagent_id: Some(child), text, .. }
            if child == "child-1" && text == "child output"
    ));

    let caps_before = caps.available_commands.clone();
    let events = map_session_update(
        SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(vec![
            AvailableCommand::new("child-cmd", "Child command"),
        ])),
        Some("child-1"),
        &mut acc,
        &mut caps,
    );
    assert!(events.is_empty());
    assert_eq!(caps.available_commands, caps_before);
}
