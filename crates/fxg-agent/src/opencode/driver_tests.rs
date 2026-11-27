//! Unit tests for the native OpenCode driver.
//!
//! Kept out of `driver.rs` so the implementation file stays focused.

use super::*;

#[test]
fn parse_server_url_reads_the_listen_line() {
    assert_eq!(
        parse_server_url("server listening on http://127.0.0.1:38219").as_deref(),
        Some("http://127.0.0.1:38219")
    );
    assert_eq!(parse_server_url("other output"), None);
}

#[test]
fn model_values_round_trip() {
    let value = parse_model_ref("opencode-go/model#high").expect("model");
    assert_eq!(value["providerID"], "opencode-go");
    assert_eq!(value["id"], "model");
    assert_eq!(value["variant"], "high");
    assert_eq!(
        model_ref_value(&value)
            .and_then(|value| value.select_value().map(str::to_owned))
            .as_deref(),
        Some("opencode-go/model#high")
    );
    assert_eq!(
        model_ref_value(&json!({
            "providerID": "opencode",
            "id": "big-pickle",
            "variant": "default",
        }))
        .and_then(|value| value.select_value().map(str::to_owned))
        .as_deref(),
        Some("opencode/big-pickle")
    );
}

#[test]
fn tool_diff_is_built_for_edit_inputs() {
    let diff = tool_diff(
        "edit",
        &json!({
            "filePath": "src/main.rs",
            "oldString": "a\n",
            "newString": "b\n"
        }),
    )
    .expect("diff");
    assert_eq!(diff.path, "src/main.rs");
    assert_eq!(diff.additions, 1);
    assert_eq!(diff.deletions, 1);
}

#[test]
fn form_constraints_and_hidden_fields_are_preserved() {
    let payload = form_to_elicitation(
        &json!({
            "form": {
                "id": "frm_2",
                "sessionID": "ses_1",
                "title": "Limits",
                "fields": [
                    {"key": "count", "type": "integer", "minimum": 1, "maximum": 10,
                     "required": true},
                    {"key": "secret", "type": "string", "hidden": true, "default": "x"},
                    {"key": "code", "type": "string", "pattern": "^[A-Z]+$",
                     "minLength": 2}
                ]
            }
        }),
        None,
    )
    .expect("elicitation");
    let UnifiedEventPayload::ElicitationRequest {
        requested_schema, ..
    } = payload
    else {
        panic!("expected a form");
    };
    let count = &requested_schema.properties["count"];
    assert_eq!(count.minimum, Some(1));
    assert_eq!(count.maximum, Some(10));
    assert_eq!(
        requested_schema.required.as_deref(),
        Some(&["count".to_owned()][..])
    );
    let secret = &requested_schema.properties["secret"];
    assert_eq!(
        secret.extra.get("hidden").map(RawJson::as_value),
        Some(&Value::Bool(true))
    );
    let code = &requested_schema.properties["code"];
    assert_eq!(
        code.extra.get("pattern").map(RawJson::as_value),
        Some(&Value::String("^[A-Z]+$".to_owned()))
    );
}

#[test]
fn failed_tool_reports_its_error_and_releases_state() {
    let runtime = test_runtime();
    let _ = map_event(
        &runtime,
        "session.tool.input.started",
        &json!({"sessionID": "ses_1", "id": "call_1", "name": "bash"}),
        None,
        None,
        &json!({"id": "evt_tool_start"}),
    );
    let _ = map_event(
        &runtime,
        "session.tool.called",
        &json!({"sessionID": "ses_1", "id": "call_1", "input": {"command": "false"}}),
        None,
        None,
        &json!({"id": "evt_tool_called"}),
    );
    let events = map_event(
        &runtime,
        "session.tool.failed",
        &json!({
            "sessionID": "ses_1",
            "id": "call_1",
            "error": {"message": "command failed"}
        }),
        None,
        None,
        &json!({"id": "evt_tool_failed"}),
    );
    assert!(matches!(
        events.as_slice(),
        [DriverEvent::Event(UnifiedEventPayload::ToolCall {
            status: ToolCallStatus::Failed,
            raw_output: Some(output),
            ..
        })] if output.contains("command failed")
    ));
    let state = runtime.state.lock().expect("state");
    assert!(state.tool_names.is_empty());
    assert!(state.tool_titles.is_empty());
    assert!(state.tool_inputs.is_empty());
}

#[test]
fn command_turns_do_not_assign_native_message_ids() {
    let runtime = Arc::new(test_runtime());
    runtime
        .state
        .lock()
        .expect("state")
        .commands
        .push("review".to_owned());
    let handle = OpenCodeSessionHandle {
        inner: Arc::new(DriverInner {
            server: Mutex::new(None),
            injected: None,
            runtimes: std::sync::Mutex::new(HashMap::new()),
            children: std::sync::Mutex::new(HashMap::new()),
            sse_running: AtomicBool::new(false),
            active_ops: std::sync::atomic::AtomicUsize::new(0),
            sync: Mutex::new(()),
            stderr: std::sync::Mutex::new(Vec::new()),
        }),
        client: OpenCodeClient::new("http://127.0.0.1:1", "test"),
        agent_session_id: "ses_1".to_owned(),
        runtime,
    };
    assert!(!handle.assigns_message_id_for(&PromptInput::text("/review")));
    assert!(!handle.assigns_message_id_for(&PromptInput::text("/compact")));
    assert!(handle.assigns_message_id_for(&PromptInput::text("/unknown")));
    assert!(handle.assigns_message_id_for(&PromptInput::text("hello")));
}

#[test]
fn inbox_emits_only_user_items_with_native_ids() {
    let runtime = test_runtime();
    let events = map_event(
        &runtime,
        "session.inbox.enqueued",
        &json!({
            "sessionID": "ses_1",
            "inboxID": "msg_1",
            "item": {
                "type": "user",
                "payload": {
                    "text": "hello",
                    "files": [{
                        "data": "aGk=",
                        "mime": "text/plain",
                        "source": { "type": "inline" },
                        "name": "a.txt"
                    }]
                }
            }
        }),
        None,
        None,
        &json!({"id": "evt_inbox_user"}),
    );
    assert!(matches!(
        events.as_slice(),
        [DriverEvent::Event(UnifiedEventPayload::UserMessage {
            text,
            attachments,
            agent_message_id: Some(id),
            ..
        })] if text == "hello"
            && attachments.len() == 1
            && attachments[0].data_b64.as_deref() == Some("aGk=")
            && id == "msg_1"
    ));

    let events = map_event(
        &runtime,
        "session.inbox.enqueued",
        &json!({
            "sessionID": "ses_1",
            "inboxID": "msg_2",
            "item": { "type": "synthetic", "payload": { "text": "hidden" } }
        }),
        None,
        None,
        &json!({"id": "evt_inbox_synthetic"}),
    );
    assert!(events.is_empty());
}

#[test]
fn child_events_keep_the_parent_status_and_metadata() {
    let runtime = test_runtime();
    let info = ChildInfo {
        root: "ses_1".to_owned(),
        parent: Some("ses_1".to_owned()),
        depth: 2,
        title: Some("Explore".to_owned()),
        status: SubagentStatus::Running,
    };

    // A child start must not make the root session busy.
    let events = map_event(
        &runtime,
        "session.execution.started",
        &json!({"sessionID": "ses_child"}),
        Some("ses_child"),
        Some(&info),
        &json!({"id": "evt_child_start"}),
    );
    assert!(events.is_empty());

    // A child rename is a subagent title update, not the root title.
    let events = map_event(
        &runtime,
        "session.renamed",
        &json!({"sessionID": "ses_child", "title": "Renamed"}),
        Some("ses_child"),
        Some(&info),
        &json!({"id": "evt_child_rename"}),
    );
    assert!(matches!(
        events.as_slice(),
        [DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
            parent_session_id: Some(parent),
            depth: 2,
            title: Some(title),
            ..
        })] if parent == "ses_1" && title == "Renamed"
    ));

    // Completion keeps the metadata stored when the child was announced.
    let info = ChildInfo {
        status: SubagentStatus::Completed,
        ..info
    };
    let events = map_event(
        &runtime,
        "session.execution.succeeded",
        &json!({"sessionID": "ses_child"}),
        Some("ses_child"),
        Some(&info),
        &json!({"id": "evt_child_done"}),
    );
    assert!(matches!(
        events.as_slice(),
        [DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
            parent_session_id: Some(parent),
            depth: 2,
            title: Some(title),
            status: SubagentStatus::Completed,
            ..
        })] if parent == "ses_1" && title == "Explore"
    ));
}

#[test]
fn location_query_encodes_the_project_directory() {
    assert_eq!(
        location_query(Path::new("/tmp/a b&c")),
        "location%5Bdirectory%5D=%2Ftmp%2Fa%20b%26c"
    );
}

#[test]
fn attachments_use_prompt_input_uris() {
    let files = attachments_to_prompt_files(&[
        fxg_protocol::common::AttachmentMeta {
            file_name: "a.txt".to_owned(),
            mime_type: "text/plain".to_owned(),
            size_bytes: 2,
            local_path: Some("/tmp/a b.txt".to_owned()),
            data_b64: None,
        },
        fxg_protocol::common::AttachmentMeta {
            file_name: "b.txt".to_owned(),
            mime_type: "text/plain".to_owned(),
            size_bytes: 2,
            local_path: None,
            data_b64: Some("aGk=".to_owned()),
        },
    ]);
    assert_eq!(files[1]["uri"], "data:text/plain;base64,aGk=");
    assert_eq!(files[1]["source"]["type"], "inline");
    assert_eq!(files[1]["name"], "b.txt");

    // The `url` crate leaves spaces unescaped on Windows; assert the shape and
    // the encoded/unescaped suffix instead of a platform-specific string.
    let uri = files[0]["uri"].as_str().expect("file uri");
    assert!(uri.starts_with("file:"), "unexpected file uri: {uri}");
    assert!(
        uri.ends_with("a b.txt") || uri.ends_with("a%20b.txt"),
        "unexpected file uri: {uri}"
    );
    assert_eq!(files[0]["name"], "a.txt");
}

#[test]
fn form_fields_become_an_elicitation_schema() {
    let payload = form_to_elicitation(
        &json!({
            "form": {
                "id": "frm_1",
                "sessionID": "ses_1",
                "title": "Pick one",
                "fields": [
                    {"key": "choice", "type": "string", "title": "Choice", "required": true,
                     "options": [{"value": "a", "label": "A"}, {"value": "b", "label": "B"}]},
                    {"key": "flag", "type": "boolean", "title": "Flag"}
                ]
            }
        }),
        None,
    )
    .expect("elicitation");
    let UnifiedEventPayload::ElicitationRequest {
        elicitation_id,
        requested_schema,
        ..
    } = payload
    else {
        panic!("expected an elicitation request");
    };
    assert_eq!(elicitation_id, "frm_1");
    assert_eq!(requested_schema.properties.len(), 2);
    assert_eq!(
        requested_schema.required.as_deref(),
        Some(&["choice".to_owned()][..])
    );
}

#[test]
fn text_and_tool_events_map_to_normalized_payloads() {
    let runtime = test_runtime();
    let events = map_event(
        &runtime,
        "session.text.ended",
        &json!({"sessionID": "ses_1", "assistantMessageID": "msg_1", "text": "hello"}),
        None,
        None,
        &json!({"id": "evt_1"}),
    );
    assert!(matches!(
        events.as_slice(),
        [DriverEvent::Event(UnifiedEventPayload::AgentMessage { text, .. })] if text == "hello"
    ));

    let events = map_event(
        &runtime,
        "session.tool.input.started",
        &json!({"sessionID": "ses_1", "id": "call_1", "name": "edit"}),
        None,
        None,
        &json!({"id": "evt_2"}),
    );
    assert!(matches!(
        events.as_slice(),
        [DriverEvent::Event(UnifiedEventPayload::ToolCall { status, .. })]
            if *status == ToolCallStatus::Pending
    ));
}

#[test]
fn usage_uses_step_tokens_and_cache_read() {
    let runtime = test_runtime();
    let events = map_event(
        &runtime,
        "session.step.ended",
        &json!({
            "sessionID": "ses_1",
            "tokens": {
                "input": 105,
                "output": 12,
                "reasoning": 1,
                "cache": { "read": 6656, "write": 0 }
            },
            "cost": 0.25
        }),
        None,
        None,
        &json!({"id": "evt_usage_step"}),
    );
    let [
        DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
            used_tokens, cost, ..
        }),
    ] = events.as_slice()
    else {
        panic!("expected usage");
    };
    assert_eq!(*used_tokens, 6774);
    assert!(cost.as_ref().is_some_and(|cost| cost.amount == 0.25));

    // `session.usage.updated` is cumulative; only its cost replaces the
    // accumulated value.
    let events = map_event(
        &runtime,
        "session.usage.updated",
        &json!({
            "sessionID": "ses_1",
            "tokens": {
                "input": 233635,
                "output": 16676,
                "reasoning": 29220,
                "cache": { "read": 9372672, "write": 0 }
            },
            "cost": 1.5
        }),
        None,
        None,
        &json!({"id": "evt_usage_session"}),
    );
    let [
        DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
            used_tokens, cost, ..
        }),
    ] = events.as_slice()
    else {
        panic!("expected usage");
    };
    assert_eq!(*used_tokens, 6774);
    assert!(cost.as_ref().is_some_and(|cost| cost.amount == 1.5));
}

#[test]
fn seen_event_ids_evicts_oldest_without_clearing_recent() {
    let mut seen = SeenEventIds::default();
    for i in 0..MAX_SEEN_EVENT_IDS {
        assert!(seen.insert(&format!("evt_{i}")));
    }
    // Adding one more evicts evt_0 while keeping recent IDs.
    assert!(seen.insert("evt_new"));
    assert!(!seen.insert("evt_new"));
    assert!(!seen.insert(&format!("evt_{}", MAX_SEEN_EVENT_IDS - 1)));
    assert!(seen.insert("evt_0"));
}

fn test_runtime() -> SessionRuntime {
    let (tx, _rx) = mpsc::unbounded_channel();
    SessionRuntime {
        fxg_session_id: "fxg".to_owned(),
        agent_session_id: "ses_1".to_owned(),
        cwd: PathBuf::from("."),
        event_tx: tx,
        state: std::sync::Mutex::new(RuntimeState::default()),
    }
}
