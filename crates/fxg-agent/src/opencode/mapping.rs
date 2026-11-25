#![allow(
    clippy::disallowed_types,
    reason = "OpenCode v2 event/capability payloads are parsed at this external boundary"
)]

use base64::Engine;
use fxg_protocol::common::AttachmentMeta;

use super::*;

/// Polls the catalog until the agent and model lists are populated.
///
/// `opencode serve` answers `/api/info` before its project catalog is ready,
/// especially for a private server. Returning an empty preview here would make
/// the node cache "no modes / no models" for a probe.
pub(super) async fn wait_for_capabilities(
    client: &OpenCodeClient,
    cwd: &Path,
    session_id: Option<&str>,
) -> Option<(UnifiedEventPayload, Vec<String>, u64)> {
    let deadline = tokio::time::Instant::now() + CATALOG_READY_TIMEOUT;
    let mut latest = None;
    loop {
        if let Ok(value) = fetch_capabilities(client, cwd, session_id).await {
            if capabilities_ready(&value.0) {
                return Some(value);
            }
            latest = Some(value);
        }
        if tokio::time::Instant::now() >= deadline {
            return latest;
        }
        tokio::time::sleep(CATALOG_POLL_INTERVAL).await;
    }
}

fn capabilities_ready(payload: &UnifiedEventPayload) -> bool {
    match payload {
        UnifiedEventPayload::CapabilitiesUpdated {
            available_modes,
            config_options,
            ..
        } => !available_modes.is_empty() && !config_options.is_empty(),
        _ => true,
    }
}

pub(super) async fn fetch_capabilities(
    client: &OpenCodeClient,
    cwd: &Path,
    session_id: Option<&str>,
) -> anyhow::Result<(UnifiedEventPayload, Vec<String>, u64)> {
    // These endpoints resolve the project configuration from `location`.
    // Without it, a shared server uses its own start directory (the first
    // session) and every other project sees the wrong agents/models/commands.
    let location = location_query(cwd);
    let agents = optional(client.get(&format!("/api/agent?{location}")).await);
    let models = optional(client.get(&format!("/api/model?{location}")).await);
    let commands = optional(client.get(&format!("/api/command?{location}")).await);
    let session = match session_id {
        Some(id) => optional(client.get(&format!("/api/session/{id}")).await),
        None => None,
    };

    let mut agent_models = HashMap::new();
    let available_modes = agents
        .as_ref()
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .map(|agents| {
            let mut modes = Vec::new();
            for agent in agents {
                if agent.get("mode").and_then(Value::as_str) == Some("subagent")
                    || agent.get("hidden").and_then(Value::as_bool) == Some(true)
                {
                    continue;
                }
                let Some(mode_id) = agent.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(model) = agent.get("model").filter(|model| !model.is_null()) {
                    agent_models.insert(mode_id.to_owned(), model.clone());
                }
                modes.push(ModeInfo {
                    mode_id: mode_id.to_owned(),
                    name: agent
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    description: agent
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
            modes
        })
        .unwrap_or_default();

    let model_options = models
        .as_ref()
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .map(|models| {
            let mut options = Vec::new();
            for model in models {
                if model.get("enabled").and_then(Value::as_bool) == Some(false) {
                    continue;
                }
                let Some(model_id) = model
                    .get("modelID")
                    .or_else(|| model.get("id"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(provider_id) = model.get("providerID").and_then(Value::as_str) else {
                    continue;
                };
                let name = model
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(model_id);
                let variants = model
                    .get("variants")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                options.push(ConfigOptionValue::Choice(ConfigOptionChoice {
                    value: format!("{provider_id}/{model_id}"),
                    name: name.to_owned(),
                    description: None,
                }));
                for variant in variants {
                    let Some(variant_id) = variant.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    options.push(ConfigOptionValue::Choice(ConfigOptionChoice {
                        value: format!("{provider_id}/{model_id}#{variant_id}"),
                        name: format!("{name} ({variant_id})"),
                        description: None,
                    }));
                }
            }
            options
        })
        .unwrap_or_default();

    let current_mode = session
        .as_ref()
        .and_then(|value| value.pointer("/data/agent"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| available_modes.first().map(|mode| mode.mode_id.clone()));

    let mut current_model_ref = session
        .as_ref()
        .and_then(|value| value.pointer("/data/model"))
        .cloned()
        .filter(|model| !model.is_null());
    if current_model_ref.is_none() {
        current_model_ref = current_mode
            .as_ref()
            .and_then(|mode| agent_models.get(mode))
            .cloned();
    }
    if current_model_ref.is_none() && session_id.is_none() {
        current_model_ref = optional(client.get(&format!("/api/model/default?{location}")).await)
            .and_then(|value| value.get("data").cloned())
            .filter(|model| !model.is_null());
    }
    let current_model = current_model_ref
        .as_ref()
        .and_then(model_ref_value)
        .map(|candidate| resolve_model_choice(&model_options, candidate));
    let context_size = current_model_ref
        .as_ref()
        .and_then(|model| {
            let provider = model.get("providerID")?.as_str()?;
            let id = model.get("id")?.as_str()?;
            models
                .as_ref()?
                .get("data")?
                .as_array()?
                .iter()
                .find(|candidate| {
                    candidate.get("providerID").and_then(Value::as_str) == Some(provider)
                        && candidate
                            .get("modelID")
                            .or_else(|| candidate.get("id"))
                            .and_then(Value::as_str)
                            == Some(id)
                })
                .and_then(|model| model.pointer("/limit/context"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);

    let config_options = if model_options.is_empty() {
        Vec::new()
    } else {
        vec![ConfigOptionInfo {
            key: "model".to_owned(),
            name: "Model".to_owned(),
            description: None,
            category: Some(ConfigOptionCategory::Model),
            current_value: current_model.unwrap_or_else(|| model_options[0].clone()),
            options: model_options,
        }]
    };

    let mut available_commands = commands
        .as_ref()
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .map(|commands| {
            commands
                .iter()
                .filter_map(|command| {
                    Some(CommandInfo {
                        name: command.get("name")?.as_str()?.to_owned(),
                        description: command
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        input_hint: command
                            .get("input")
                            .and_then(|input| input.get("hint"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !available_commands
        .iter()
        .any(|command| command.name == COMPACT_COMMAND)
    {
        available_commands.push(CommandInfo {
            name: COMPACT_COMMAND.to_owned(),
            description: "Compact the conversation context".to_owned(),
            input_hint: None,
        });
    }
    let command_names = available_commands
        .iter()
        .map(|command| command.name.clone())
        .collect();

    Ok((
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            available_commands,
            config_options,
            fork_scope: Some(ForkScope::Message),
            native_features: NATIVE_SESSION_FEATURES.to_vec(),
        },
        command_names,
        context_size,
    ))
}

/// Re-synchronizes running state and pending interactive requests after an SSE
/// reconnect. `permission.asked` / `form.created` are not durable, so without
/// this a reconnecting client can leave an agent waiting forever.
async fn active_sessions_or_empty(client: &OpenCodeClient) -> HashSet<String> {
    match client.active_sessions().await {
        Ok(active) => active,
        Err(err) => {
            debug!("failed to read active OpenCode sessions: {err:#}");
            HashSet::new()
        }
    }
}

pub(super) async fn resync_all(inner: &Arc<DriverInner>, client: &OpenCodeClient) {
    let active = active_sessions_or_empty(client).await;
    for runtime in inner.runtimes() {
        if active.contains(&runtime.agent_session_id) {
            runtime.set_busy(true);
            runtime.emit_event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            });
        }
        resync_pending(client, &runtime, &runtime.agent_session_id, None).await;
        for child in inner.children_for_root(&runtime.agent_session_id) {
            resync_pending(client, &runtime, &child, Some(child.as_str())).await;
        }
    }
}

/// Resynchronizes one session (or child session) without touching the root
/// status from a child start.
pub(super) async fn resync_session(
    inner: &Arc<DriverInner>,
    client: &OpenCodeClient,
    runtime: &Arc<SessionRuntime>,
) {
    if active_sessions_or_empty(client)
        .await
        .contains(&runtime.agent_session_id)
    {
        runtime.set_busy(true);
        runtime.emit_event(UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Running,
            error_message: None,
        });
    }
    resync_pending(client, runtime, &runtime.agent_session_id, None).await;
    for child in inner.children_for_root(&runtime.agent_session_id) {
        resync_pending(client, runtime, &child, Some(child.as_str())).await;
    }
}

async fn resync_pending(
    client: &OpenCodeClient,
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
    subagent_id: Option<&str>,
) {
    if let Ok(permissions) = client.pending_permissions(session_id).await {
        for permission in permissions {
            let events = map_event(
                runtime,
                "permission.asked",
                &permission,
                subagent_id,
                None,
                &Value::Null,
            );
            runtime.emit(events);
        }
    }
    if let Ok(forms) = client.pending_forms(session_id).await {
        for form in forms {
            let data = json!({ "form": form });
            let events = map_event(
                runtime,
                "form.created",
                &data,
                subagent_id,
                None,
                &Value::Null,
            );
            runtime.emit(events);
        }
    }
}

pub(super) fn model_ref_value(value: &Value) -> Option<ConfigOptionValue> {
    let id = value.get("id").and_then(Value::as_str)?;
    let provider = value.get("providerID").and_then(Value::as_str)?;
    let variant = value.get("variant").and_then(Value::as_str);
    let wire = match variant {
        // OpenCode fills in `"default"` when no variant was requested, whereas
        // selectable choices only include explicit variants (or none at all).
        Some(variant) if !variant.is_empty() && variant != "default" => {
            format!("{provider}/{id}#{variant}")
        }
        _ => format!("{provider}/{id}"),
    };
    Some(ConfigOptionValue::Choice(ConfigOptionChoice {
        name: wire.clone(),
        value: wire,
        description: None,
    }))
}

fn resolve_model_choice(
    options: &[ConfigOptionValue],
    candidate: ConfigOptionValue,
) -> ConfigOptionValue {
    let Some(wire) = candidate.select_value() else {
        return candidate;
    };
    if let Some(exact) = options
        .iter()
        .find(|choice| choice.select_value() == Some(wire))
    {
        return exact.clone();
    }
    if let Some((base, _)) = wire.split_once('#')
        && let Some(fallback) = options
            .iter()
            .find(|choice| choice.select_value() == Some(base))
    {
        return fallback.clone();
    }
    candidate
}

pub(super) fn parse_model_ref(value: &str) -> Option<Value> {
    let (model, variant) = match value.split_once('#') {
        Some((model, variant)) => (model, Some(variant)),
        None => (value, None),
    };
    let (provider_id, id) = model.split_once('/')?;
    let mut body = json!({ "providerID": provider_id, "id": id });
    if let Some(variant) = variant {
        body["variant"] = Value::String(variant.to_owned());
    }
    Some(body)
}

pub(super) async fn handle_raw_event(inner: &Arc<DriverInner>, value: &Value) {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind == "server.connected" {
        // The connection is ready: backfill the window between the session
        // start and this subscription, then resync pending requests.
        let inner = Arc::clone(inner);
        tokio::spawn(async move {
            sync_all(&inner).await;
        });
        return;
    }
    if kind == "log.synced" {
        return;
    }
    apply_raw_event(inner, value).await;
}

/// Applies one event to its runtime. Kept separate from `handle_raw_event` so
/// backfill can reuse the exact live path without recursing into the connection
/// sync.
pub(super) async fn apply_raw_event(inner: &Arc<DriverInner>, value: &Value) {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    if let Some(session_id) = event_session_id(kind, &data) {
        if kind == "session.created" {
            // Child creation is handled outside `map_event`; dedupe it here so
            // a replayed log entry does not reset a completed child to Running.
            if let (Some(event_id), Some(parent)) = (
                value.get("id").and_then(Value::as_str),
                data.get("parentID").and_then(Value::as_str),
            ) && let Some((parent_runtime, _)) = inner.runtime_for(parent)
                && !parent_runtime
                    .state
                    .lock()
                    .expect("state poisoned")
                    .seen_event_ids
                    .insert(event_id)
            {
                return;
            }
            handle_session_created(inner, &data, session_id).await;
            return;
        }
        if let Some((runtime, subagent_id)) = inner.runtime_for(session_id) {
            if let Some(child) = subagent_id.as_deref() {
                let status = match kind {
                    "session.execution.started" => Some(SubagentStatus::Running),
                    "session.execution.succeeded" | "session.idle" => {
                        Some(SubagentStatus::Completed)
                    }
                    "session.execution.interrupted" => Some(SubagentStatus::Cancelled),
                    "session.execution.failed" | "session.error" => Some(SubagentStatus::Failed),
                    _ => None,
                };
                if let Some(status) = status {
                    inner.update_child_status(child, status);
                }
            }
            let child_info = subagent_id
                .as_deref()
                .and_then(|child| inner.child_info(child));
            let events = map_event(
                &runtime,
                kind,
                &data,
                subagent_id.as_deref(),
                child_info.as_ref(),
                value,
            );
            runtime.emit(events);
            if subagent_id.is_none()
                && matches!(
                    kind,
                    "session.agent.selected"
                        | "session.model.selected"
                        | "session.execution.succeeded"
                )
                && let Some(server) = inner.server().await
            {
                runtime.refresh_capabilities(&server.client).await;
            }
        }
    }
}

pub(super) fn event_session_id<'a>(kind: &str, data: &'a Value) -> Option<&'a str> {
    data.get("sessionID")
        .and_then(Value::as_str)
        .or_else(|| {
            data.get("payload")
                .and_then(|payload| payload.get("sessionID"))
                .and_then(Value::as_str)
        })
        .or_else(|| data.pointer("/form/sessionID").and_then(Value::as_str))
        .or_else(|| {
            if kind.starts_with("form.") {
                data.get("sessionID").and_then(Value::as_str)
            } else {
                None
            }
        })
}

pub(super) async fn handle_session_created(
    inner: &Arc<DriverInner>,
    data: &Value,
    session_id: &str,
) {
    let parent = data.get("parentID").and_then(Value::as_str);
    let Some(parent) = parent else {
        return;
    };
    let Some((runtime, _)) = inner.runtime_for(parent) else {
        return;
    };
    let depth = {
        let children = inner.children.lock().expect("children poisoned");
        children
            .get(parent)
            .map_or(1, |parent_info| parent_info.depth + 1)
    };
    let title = data.get("title").and_then(Value::as_str).map(str::to_owned);
    {
        let mut children = inner.children.lock().expect("children poisoned");
        children.insert(
            session_id.to_owned(),
            ChildInfo {
                root: runtime.agent_session_id.clone(),
                parent: Some(parent.to_owned()),
                depth,
                title: title.clone(),
                status: SubagentStatus::Running,
            },
        );
    }
    runtime.emit_event(UnifiedEventPayload::SubagentUpdated {
        child_session_id: session_id.to_owned(),
        parent_session_id: Some(parent.to_owned()),
        depth,
        title,
        description: None,
        status: SubagentStatus::Running,
    });
}

pub(super) fn map_event(
    runtime: &SessionRuntime,
    kind: &str,
    data: &Value,
    subagent_id: Option<&str>,
    child_info: Option<&ChildInfo>,
    raw: &Value,
) -> Vec<DriverEvent> {
    if let Some(id) = raw.get("id").and_then(Value::as_str) {
        let mut state = runtime.state.lock().expect("state poisoned");
        if !state.seen_event_ids.insert(id) {
            return Vec::new();
        }
        if let Some(seq) = raw.pointer("/durable/seq").and_then(Value::as_u64) {
            state.durable_seq.insert(
                data.get("sessionID")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                seq,
            );
        }
    }

    let text_of = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let scope = subagent_id.map(str::to_owned);

    match kind {
        "session.created" => Vec::new(),
        "session.renamed" | "session.title.updated" => {
            let title = data
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|title| !title.is_empty());
            let Some(title) = title else {
                return Vec::new();
            };
            if let Some(child) = subagent_id {
                let (parent, depth, status) = child_info
                    .map_or((None, 1, SubagentStatus::Running), |info| {
                        (info.parent.clone(), info.depth, info.status.clone())
                    });
                return vec![DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                    child_session_id: child.to_owned(),
                    parent_session_id: parent,
                    depth,
                    title: Some(title.to_owned()),
                    description: None,
                    status,
                })];
            }
            vec![DriverEvent::Event(
                UnifiedEventPayload::SessionTitleChanged {
                    title: title.to_owned(),
                },
            )]
        }
        "session.execution.started" => {
            // A child run must not make the parent look busy. The child card
            // already reflects its state through SubagentUpdated.
            if subagent_id.is_some() {
                return Vec::new();
            }
            runtime.set_busy(true);
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            })]
        }
        "session.execution.succeeded" | "session.execution.interrupted" | "session.idle" => {
            if let Some(child) = subagent_id {
                let status = if kind == "session.execution.interrupted" {
                    SubagentStatus::Cancelled
                } else {
                    child_info.map_or(SubagentStatus::Completed, |info| info.status.clone())
                };
                let (parent, depth, title) = child_info.map_or((None, 1, None), |info| {
                    (info.parent.clone(), info.depth, info.title.clone())
                });
                return vec![DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                    child_session_id: child.to_owned(),
                    parent_session_id: parent,
                    depth,
                    title,
                    description: None,
                    status,
                })];
            }
            let turn_end = {
                let mut state = runtime.state.lock().expect("state poisoned");
                if kind == "session.execution.succeeded" {
                    state.take_turn_stop_reason()
                } else if kind == "session.execution.interrupted" {
                    state.finish = None;
                    state.step_error = None;
                    None
                } else {
                    None
                }
            };
            runtime.set_busy(false);
            clear_tool_state(runtime);
            let mut events = Vec::new();
            if let Some(reason) = turn_end {
                events.push(DriverEvent::Event(UnifiedEventPayload::TurnEnded {
                    reason,
                }));
            }
            events.push(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Idle,
                error_message: None,
            }));
            events
        }
        "session.execution.failed" => {
            let error_type = data.pointer("/error/type").and_then(Value::as_str);
            if let Some(child) = subagent_id {
                let (parent, depth, title) = child_info.map_or((None, 1, None), |info| {
                    (info.parent.clone(), info.depth, info.title.clone())
                });
                return vec![DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                    child_session_id: child.to_owned(),
                    parent_session_id: parent,
                    depth,
                    title,
                    description: None,
                    status: SubagentStatus::Failed,
                })];
            }
            let turn_end = {
                let mut state = runtime.state.lock().expect("state poisoned");
                let step_error = state.step_error.take();
                state.finish = None;
                turn_stop_reason(None, error_type.or(step_error.as_deref()))
            };
            runtime.set_busy(false);
            clear_tool_state(runtime);
            if let Some(reason) = turn_end {
                return vec![
                    DriverEvent::Event(UnifiedEventPayload::TurnEnded { reason }),
                    DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                        status: SessionStatus::Idle,
                        error_message: None,
                    }),
                ];
            }
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Error,
                error_message: Some(error_message(data, "OpenCode execution failed")),
            })]
        }
        "session.retry.scheduled" => {
            if subagent_id.is_some() {
                return Vec::new();
            }
            let attempt = data.get("attempt").and_then(Value::as_u64).unwrap_or(0);
            vec![DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: Some(format!("OpenCode is retrying (attempt {attempt})")),
            })]
        }
        "session.error" => {
            if let Some(child) = subagent_id {
                let (parent, depth, title) = child_info.map_or((None, 1, None), |info| {
                    (info.parent.clone(), info.depth, info.title.clone())
                });
                return vec![DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                    child_session_id: child.to_owned(),
                    parent_session_id: parent,
                    depth,
                    title,
                    description: None,
                    status: SubagentStatus::Failed,
                })];
            }
            vec![DriverEvent::Failed {
                message: error_message(data, "OpenCode session error"),
            }]
        }
        "session.compaction.started"
        | "session.compaction.ended"
        | "session.compaction.failed"
        | "session.compaction.delta" => {
            if subagent_id.is_some() {
                return Vec::new();
            }
            match kind {
                "session.compaction.started" => {
                    vec![DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                        status: fxg_protocol::common::CompactionStatus::Started,
                        detail: None,
                    })]
                }
                "session.compaction.ended" => {
                    vec![DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                        status: fxg_protocol::common::CompactionStatus::Completed,
                        detail: None,
                    })]
                }
                "session.compaction.failed" => {
                    vec![DriverEvent::Event(UnifiedEventPayload::CompactionUpdated {
                        status: fxg_protocol::common::CompactionStatus::Failed,
                        detail: data
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .or_else(|| data.get("error").map(ToString::to_string)),
                    })]
                }
                _ => Vec::new(),
            }
        }
        "session.text.delta" => vec![DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta {
            message_id: text_of("assistantMessageID"),
            text_delta: text_of("delta"),
            subagent_id: scope,
        })],
        "session.text.ended" => vec![DriverEvent::Event(UnifiedEventPayload::AgentMessage {
            message_id: text_of("assistantMessageID"),
            text: text_of("text"),
            is_complete: true,
            subagent_id: scope,
        })],
        "session.reasoning.delta" => {
            vec![DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta {
                thought_id: reasoning_id(data),
                text_delta: text_of("delta"),
                subagent_id: scope,
            })]
        }
        "session.reasoning.ended" => vec![DriverEvent::Event(UnifiedEventPayload::AgentThought {
            thought_id: reasoning_id(data),
            text: text_of("text"),
            is_complete: true,
            subagent_id: scope,
        })],
        "session.tool.input.started" => {
            let tool_call_id = text_of("id");
            let name = text_of("name");
            runtime
                .state
                .lock()
                .expect("state poisoned")
                .tool_names
                .insert(tool_call_id.clone(), name.clone());
            vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title: name.clone(),
                kind: tool_kind(&name),
                status: ToolCallStatus::Pending,
                locations: Vec::new(),
                diff: None,
                raw_output: None,
                terminal_id: None,
                subagent_id: scope,
            })]
        }
        "session.tool.input.ended" => {
            let id = text_of("id");
            let text = text_of("text");
            runtime
                .state
                .lock()
                .expect("state poisoned")
                .tool_inputs
                .insert(id, Value::String(text));
            Vec::new()
        }
        "session.tool.called" => {
            let tool_call_id = text_of("id");
            let input = data.get("input").cloned().unwrap_or(Value::Null);
            let name = {
                let state = runtime.state.lock().expect("state poisoned");
                state
                    .tool_names
                    .get(&tool_call_id)
                    .cloned()
                    .unwrap_or_else(|| "tool".to_owned())
            };
            let title = tool_title(&name, &input);
            let kind = tool_kind(&name);
            let locations = tool_locations(&input);
            let diff = tool_diff(&name, &input);
            let plan = todo_plan_entries(&name, &input);
            {
                let mut state = runtime.state.lock().expect("state poisoned");
                state
                    .tool_titles
                    .insert(tool_call_id.clone(), title.clone());
                state.tool_inputs.insert(tool_call_id.clone(), input);
            }
            let mut events = vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title,
                kind,
                status: ToolCallStatus::InProgress,
                locations,
                diff,
                raw_output: None,
                terminal_id: None,
                subagent_id: scope.clone(),
            })];
            if let Some(entries) = plan {
                events.push(DriverEvent::Event(UnifiedEventPayload::PlanUpdate {
                    entries,
                    subagent_id: scope,
                }));
            }
            events
        }
        "session.tool.progress" => {
            // Progress is ephemeral. Emitting a persisted `ToolCall` for every
            // metadata tick bloats the event log; the UI already has the tool
            // card and merges this delta into it.
            vec![DriverEvent::Delta(StreamDeltaPayload::ToolCallProgress {
                tool_call_id: text_of("id"),
                status: ToolCallStatus::InProgress,
                raw_output_delta: None,
            })]
        }
        "session.tool.success" | "session.tool.failed" => {
            let tool_call_id = text_of("id");
            let failed = kind == "session.tool.failed";
            let mut state = runtime.state.lock().expect("state poisoned");
            let name = state
                .tool_names
                .remove(&tool_call_id)
                .unwrap_or_else(|| "tool".to_owned());
            let title = state
                .tool_titles
                .remove(&tool_call_id)
                .unwrap_or_else(|| name.clone());
            let input = state
                .tool_inputs
                .remove(&tool_call_id)
                .unwrap_or(Value::Null);
            drop(state);
            let raw_output = match (tool_output(data), data.pointer("/error/message")) {
                (Some(output), Some(error)) if failed => {
                    Some(format!("{output}\n{}", error.as_str().unwrap_or_default()))
                }
                (output, Some(error)) if failed => error.as_str().map(str::to_owned).or(output),
                (output, _) => output,
            };
            vec![DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                title,
                kind: tool_kind(&name),
                status: if failed {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                },
                locations: tool_locations(&input),
                diff: tool_diff(&name, &input),
                raw_output,
                terminal_id: None,
                subagent_id: scope,
            })]
        }
        "session.step.ended" | "session.step.failed" => {
            if scope.is_none() {
                let mut state = runtime.state.lock().expect("state poisoned");
                if kind == "session.step.ended" {
                    state.finish = data
                        .get("finish")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    state.step_error = None;
                } else {
                    state.step_error = data
                        .pointer("/error/type")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
            }
            step_usage_events(runtime, data, scope)
        }
        "session.usage.updated" => {
            // `session.usage.updated` carries cumulative session totals. It is
            // not the current context, so reuse the last step's prompt +
            // completion tokens and only take the cumulative cost from here.
            let cumulative_tokens = data.get("tokens").cloned().unwrap_or(Value::Null);
            let cumulative_used = token_usage_used(&cumulative_tokens);
            let cumulative_cost = data.get("cost").and_then(Value::as_f64);
            let context_size = runtime.state.lock().expect("state poisoned").context_size;
            let (used_tokens, cost) = {
                let mut state = runtime.state.lock().expect("state poisoned");
                let entry = state.usage.entry(scope.clone()).or_default();
                if entry.used_tokens == 0 {
                    entry.used_tokens = cumulative_used;
                }
                if cumulative_cost.is_some() {
                    entry.cost = cumulative_cost;
                }
                (entry.used_tokens, entry.cost)
            };
            vec![DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
                used_tokens,
                context_size,
                cost: cost.map(|amount| UsageCost {
                    amount,
                    currency: "USD".to_owned(),
                }),
                subagent_id: scope,
            })]
        }
        "permission.asked" => vec![
            DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::WaitingPermission,
                error_message: None,
            }),
            DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
                request_id: text_of("id"),
                tool_name: text_of("action"),
                summary: permission_summary(data),
                options: permission_options(),
                details: RawJson::new(data.clone()),
                subagent_id: scope,
            }),
        ],
        "permission.replied" => vec![
            DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            }),
            DriverEvent::Event(UnifiedEventPayload::PermissionResolved {
                request_id: text_of("requestID"),
                selected_option_id: text_of("reply"),
                resolved_by: fxg_protocol::common::ResolvedBy::Cli,
            }),
        ],
        "form.created" => match form_to_elicitation(data, scope.clone()) {
            Some(payload) => vec![
                DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::WaitingInput,
                    error_message: None,
                }),
                DriverEvent::Event(payload),
            ],
            None => Vec::new(),
        },
        "form.replied" => vec![
            DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            }),
            DriverEvent::Event(UnifiedEventPayload::ElicitationResolved {
                elicitation_id: text_of("id"),
                action: ElicitationAction::Accept,
                content: form_answer(data),
                resolved_by: fxg_protocol::common::ResolvedBy::Cli,
            }),
        ],
        "form.cancelled" => vec![
            DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Running,
                error_message: None,
            }),
            DriverEvent::Event(UnifiedEventPayload::ElicitationResolved {
                elicitation_id: text_of("id"),
                action: ElicitationAction::Cancel,
                content: None,
                resolved_by: fxg_protocol::common::ResolvedBy::Cli,
            }),
        ],
        "session.agent.selected" | "session.model.selected" => Vec::new(),
        "session.inbox.enqueued" => {
            // `UserMessage` has no subagent_id; a child prompt must not appear
            // as a root user turn.
            if subagent_id.is_some() {
                return Vec::new();
            }
            // The inbox item is a union (user / synthetic / compaction / move).
            // Only a user prompt becomes a fxg UserMessage.
            if data.pointer("/item/type").and_then(Value::as_str) != Some("user") {
                return Vec::new();
            }
            let inbox_id = text_of("inboxID");
            let text = data
                .pointer("/item/payload/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let attachments = attachments_from_prompt_files(data.pointer("/item/payload/files"));
            {
                let mut state = runtime.state.lock().expect("state poisoned");
                if state.pending_commands > 0 {
                    state.pending_commands -= 1;
                    return Vec::new();
                }
                if state.self_messages.contains(&inbox_id) {
                    state.self_messages.remove(&inbox_id);
                    return Vec::new();
                }
            }
            if text.is_empty() && attachments.is_empty() {
                return Vec::new();
            }
            vec![DriverEvent::Event(UnifiedEventPayload::UserMessage {
                text,
                attachments,
                client_source: ClientSource::Cli,
                // The inbox ID is the OpenCode user message ID, so external TUI
                // prompts can be targets of revert / fork as well.
                agent_message_id: (!inbox_id.is_empty()).then_some(inbox_id),
            })]
        }
        "session.synthetic" => Vec::new(),
        _ => Vec::new(),
    }
}

fn clear_tool_state(runtime: &SessionRuntime) {
    let mut state = runtime.state.lock().expect("state poisoned");
    state.tool_names.clear();
    state.tool_titles.clear();
    state.tool_inputs.clear();
}

fn step_usage_events(
    runtime: &SessionRuntime,
    data: &Value,
    scope: Option<String>,
) -> Vec<DriverEvent> {
    let Some(tokens) = data.get("tokens") else {
        return Vec::new();
    };
    let used_tokens = token_usage_used(tokens);
    let step_cost = data.get("cost").and_then(Value::as_f64);
    let context_size = runtime.state.lock().expect("state poisoned").context_size;
    let cost = {
        let mut state = runtime.state.lock().expect("state poisoned");
        let entry = state.usage.entry(scope.clone()).or_default();
        entry.used_tokens = used_tokens;
        if let Some(step_cost) = step_cost {
            // `session.usage.updated` replaces this with the cumulative total
            // when it arrives; summing steps keeps backfill useful.
            entry.cost = Some(entry.cost.unwrap_or(0.0) + step_cost);
        }
        entry.cost
    };
    vec![DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
        used_tokens,
        context_size,
        cost: cost.map(|amount| UsageCost {
            amount,
            currency: "USD".to_owned(),
        }),
        subagent_id: scope,
    })]
}

/// Tokens occupying the context after a step.
///
/// `input` excludes cached prompt tokens in OpenCode v2, so `cache.read` must be
/// added as well. Output and reasoning are the generated part of the context.
fn token_usage_used(tokens: &Value) -> u64 {
    let field = |pointer: &str| tokens.pointer(pointer).and_then(Value::as_u64).unwrap_or(0);
    field("/input") + field("/cache/read") + field("/output") + field("/reasoning")
}

pub(super) fn reasoning_id(data: &Value) -> String {
    let message = data
        .get("assistantMessageID")
        .and_then(Value::as_str)
        .unwrap_or("reasoning");
    let ordinal = data.get("ordinal").and_then(Value::as_u64).unwrap_or(0);
    format!("{message}:reasoning:{ordinal}")
}

/// Extracts the plan from a `todowrite` tool call.
///
/// OpenCode v2 sends the full todo list as the tool input (and an empty list
/// clears it). Providers occasionally encode the input as a JSON string, so
/// both shapes are accepted.
pub(super) fn todo_plan_entries(name: &str, input: &Value) -> Option<Vec<PlanEntry>> {
    if name != "todowrite" {
        return None;
    }
    let todos = match input.get("todos") {
        Some(Value::Array(todos)) => todos.clone(),
        Some(Value::String(encoded)) => {
            let parsed: Value = match serde_json::from_str(encoded) {
                Ok(value) => value,
                Err(_) => return None,
            };
            match parsed {
                Value::Array(todos) => todos,
                other => other.get("todos")?.as_array()?.clone(),
            }
        }
        _ => return None,
    };
    Some(
        todos
            .iter()
            .enumerate()
            .filter_map(|(index, todo)| {
                let title = todo
                    .get("content")
                    .or_else(|| todo.get("title"))
                    .or_else(|| todo.get("text"))
                    .and_then(Value::as_str)?;
                let id = todo
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("plan-{index}"));
                Some(PlanEntry {
                    id,
                    title: title.to_owned(),
                    status: plan_status(todo.get("status").and_then(Value::as_str)),
                })
            })
            .collect(),
    )
}

fn plan_status(status: Option<&str>) -> PlanEntryStatus {
    match status {
        Some("in_progress") => PlanEntryStatus::InProgress,
        Some("completed") => PlanEntryStatus::Completed,
        _ => PlanEntryStatus::Pending,
    }
}

pub(super) fn tool_kind(name: &str) -> ToolCallKind {
    match name {
        "read" | "list" | "glob" | "grep" | "find" => ToolCallKind::Read,
        "edit" | "write" | "patch" | "multiedit" => ToolCallKind::Edit,
        "bash" | "shell" | "terminal" => ToolCallKind::Execute,
        "webfetch" | "websearch" | "task" | "todowrite" | "todoread" => ToolCallKind::Search,
        _ => ToolCallKind::Other,
    }
}

pub(super) fn tool_title(name: &str, input: &Value) -> String {
    match name {
        "bash" | "shell" => input
            .get("command")
            .and_then(Value::as_str)
            .map(|command| format!("$ {command}"))
            .unwrap_or_else(|| name.to_owned()),
        "read" | "edit" | "write" => input
            .get("filePath")
            .or_else(|| input.get("path"))
            .and_then(Value::as_str)
            .map(|path| format!("{name}: {path}"))
            .unwrap_or_else(|| name.to_owned()),
        "grep" | "glob" | "find" => input
            .get("pattern")
            .and_then(Value::as_str)
            .map(|pattern| format!("{name}: {pattern}"))
            .unwrap_or_else(|| name.to_owned()),
        "todowrite" => "Update todo list".to_owned(),
        _ => name.to_owned(),
    }
}

pub(super) fn tool_locations(input: &Value) -> Vec<String> {
    let mut locations = Vec::new();
    for key in ["filePath", "path", "filename"] {
        if let Some(path) = input.get(key).and_then(Value::as_str) {
            locations.push(path.to_owned());
        }
    }
    if let Some(paths) = input.get("paths").and_then(Value::as_array) {
        locations.extend(paths.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    locations.sort();
    locations.dedup();
    locations
}

pub(super) fn tool_diff(name: &str, input: &Value) -> Option<fxg_protocol::common::FileDiff> {
    let (path, old_text, new_text) = match name {
        "edit" => {
            let path = input.get("filePath").or_else(|| input.get("path"))?;
            let path = path.as_str()?;
            let old = input
                .get("oldString")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let new = input
                .get("newString")
                .and_then(Value::as_str)
                .unwrap_or_default();
            (path, old, new)
        }
        "write" => {
            let path = input.get("filePath").or_else(|| input.get("path"))?;
            let path = path.as_str()?;
            let new = input
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            (path, "", new)
        }
        _ => return None,
    };
    let patch = diffy::create_patch(old_text, new_text);
    let unified = patch.to_string();
    let mut additions = 0;
    let mut deletions = 0;
    for line in unified.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        }
    }
    Some(fxg_protocol::common::FileDiff {
        path: path.to_owned(),
        old_text: Some(old_text.to_owned()),
        new_text: Some(new_text.to_owned()),
        unified_diff: unified,
        additions,
        deletions,
    })
}

pub(super) fn tool_output(data: &Value) -> Option<String> {
    let content = data.get("content").and_then(Value::as_array)?;
    let mut texts = Vec::new();
    for item in content {
        if item.get("type").and_then(Value::as_str) == Some("text")
            && let Some(text) = item.get("text").and_then(Value::as_str)
        {
            texts.push(text.to_owned());
        }
    }
    if texts.is_empty() {
        data.get("metadata")
            .filter(|value| !value.is_null())
            .map(ToString::to_string)
    } else {
        Some(texts.join("\n"))
    }
}

pub(super) fn permission_summary(data: &Value) -> String {
    if let Some(message) = data.get("message").and_then(Value::as_str)
        && !message.trim().is_empty()
    {
        return message.to_owned();
    }
    let action = data
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("permission");
    let resources = data
        .get("resources")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if resources.is_empty() {
        action.to_owned()
    } else {
        format!("{action}: {resources}")
    }
}

pub(super) fn permission_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption {
            option_id: "once".to_owned(),
            name: "Allow once".to_owned(),
            kind: PermissionOptionKind::AllowOnce,
        },
        PermissionOption {
            option_id: "always".to_owned(),
            name: "Always allow".to_owned(),
            kind: PermissionOptionKind::AllowAlways,
        },
        PermissionOption {
            option_id: "reject".to_owned(),
            name: "Reject".to_owned(),
            kind: PermissionOptionKind::RejectOnce,
        },
    ]
}

pub(super) fn form_to_elicitation(
    data: &Value,
    subagent_id: Option<String>,
) -> Option<UnifiedEventPayload> {
    let form = data.get("form")?;
    let form_id = form.get("id")?.as_str()?.to_owned();
    let session_id = form.get("sessionID").and_then(Value::as_str);
    session_id?;
    let title = form
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Question")
        .to_owned();
    let fields = form.get("fields").and_then(Value::as_array)?;
    let mut properties = BTreeMap::new();
    let mut required = Vec::new();
    for field in fields {
        let Some(key) = field.get("key").and_then(Value::as_str) else {
            continue;
        };
        let hidden = field.get("hidden").and_then(Value::as_bool) == Some(true);
        let kind = field
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("string");
        let field_title = field
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let description = field
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let default = field.get("default").cloned().map(RawJson::new);
        let mut property = ElicitationProperty {
            kind: ElicitationPropertyKind::String,
            title: field_title,
            description,
            default,
            r#enum: Vec::new(),
            one_of: Vec::new(),
            items: None,
            minimum: None,
            maximum: None,
            extra: BTreeMap::new(),
        };
        if hidden {
            property
                .extra
                .insert("hidden".to_owned(), RawJson::new(Value::Bool(true)));
        }
        match kind {
            "integer" => property.kind = ElicitationPropertyKind::Integer,
            "number" => property.kind = ElicitationPropertyKind::Number,
            "boolean" => property.kind = ElicitationPropertyKind::Boolean,
            "multiselect" => {
                property.kind = ElicitationPropertyKind::Array;
                let options = field
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(|option| option.get("value")?.as_str().map(str::to_owned))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                property.items = Some(ElicitationArrayItems {
                    kind: ElicitationPropertyKind::String,
                    r#enum: options,
                    extra: BTreeMap::new(),
                });
            }
            _ => {
                if let Some(options) = field.get("options").and_then(Value::as_array) {
                    property.one_of = options
                        .iter()
                        .filter_map(|option| {
                            Some(ElicitationOption {
                                const_value: option.get("value")?.as_str()?.to_owned(),
                                title: option
                                    .get("label")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                extra: BTreeMap::new(),
                            })
                        })
                        .collect();
                }
            }
        }
        if matches!(kind, "integer" | "number") {
            for (key, target) in [
                ("minimum", &mut property.minimum),
                ("maximum", &mut property.maximum),
            ] {
                if let Some(value) = field.get(key) {
                    if let Some(value) = value.as_i64() {
                        *target = Some(value);
                    } else if let Some(value) = value.as_f64() {
                        // The normalized schema only has integer bounds; keep a
                        // fractional bound in `extra` for the UI.
                        property
                            .extra
                            .insert(key.to_owned(), RawJson::new(json!(value)));
                    }
                }
            }
        }
        for key in [
            "format",
            "minLength",
            "maxLength",
            "pattern",
            "placeholder",
            "custom",
            "when",
            "minItems",
            "maxItems",
            "external",
            "url",
        ] {
            if let Some(value) = field.get(key) {
                property
                    .extra
                    .insert(key.to_owned(), RawJson::new(value.clone()));
            }
        }
        if !hidden && field.get("required").and_then(Value::as_bool) == Some(true) {
            required.push(key.to_owned());
        }
        properties.insert(key.to_owned(), property);
    }
    if properties.is_empty() {
        return None;
    }
    Some(UnifiedEventPayload::ElicitationRequest {
        elicitation_id: form_id,
        message: title,
        mode: fxg_protocol::common::ElicitationMode::Form,
        requested_schema: ElicitationSchema {
            schema_type: ElicitationSchemaType::Object,
            title: None,
            description: None,
            properties,
            required: (!required.is_empty()).then_some(required),
            extra: BTreeMap::new(),
        },
        tool_call_id: None,
        subagent_id,
    })
}

pub(super) fn form_answer(data: &Value) -> Option<ElicitationContent> {
    let answer = data.get("answer")?.as_object()?;
    let mut content = ElicitationContent::new();
    for (key, value) in answer {
        content.insert(key.clone(), json_to_elicitation_value(value));
    }
    Some(content)
}

pub(super) fn elicitation_value_to_json(value: &ElicitationValue) -> Value {
    match value {
        ElicitationValue::String(value) => Value::String(value.clone()),
        ElicitationValue::Integer(value) => json!(value),
        ElicitationValue::Number(value) => json!(value),
        ElicitationValue::Boolean(value) => json!(value),
        ElicitationValue::StringArray(values) => json!(values),
        ElicitationValue::Unknown(value) => value.as_value().clone(),
    }
}

pub(super) fn json_to_elicitation_value(value: &Value) -> ElicitationValue {
    match value {
        Value::String(value) => ElicitationValue::String(value.clone()),
        Value::Bool(value) => ElicitationValue::Boolean(*value),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                ElicitationValue::Integer(value)
            } else if let Some(value) = value.as_f64() {
                ElicitationValue::Number(value)
            } else {
                ElicitationValue::Unknown(RawJson::new(Value::Number(value.clone())))
            }
        }
        Value::Array(values) => ElicitationValue::StringArray(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        ),
        _ => ElicitationValue::Unknown(RawJson::new(value.clone())),
    }
}

/// Converts OpenCode's materialized `Prompt.FileAttachment` values back into
/// fxg attachment metadata (used for TUI/imported user messages).
pub(super) fn attachments_from_prompt_files(value: Option<&Value>) -> Vec<AttachmentMeta> {
    let Some(files) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    files
        .iter()
        .filter_map(|file| {
            let file_name = file
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("attachment")
                .to_owned();
            let mime_type = file
                .get("mime")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_owned();
            if let Some(data) = file.get("data").and_then(Value::as_str) {
                let size_bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_or(0, |bytes| bytes.len() as u64);
                return Some(AttachmentMeta {
                    file_name,
                    mime_type,
                    size_bytes,
                    local_path: None,
                    data_b64: Some(data.to_owned()),
                });
            }
            let uri = file
                .pointer("/source/uri")
                .and_then(Value::as_str)
                .or_else(|| file.get("uri").and_then(Value::as_str))?;
            let local_path = uri.strip_prefix("file://").unwrap_or(uri).to_owned();
            Some(AttachmentMeta {
                file_name,
                mime_type,
                size_bytes: 0,
                local_path: Some(local_path),
                data_b64: None,
            })
        })
        .collect()
}

/// Converts attachments to the `PromptInput.FileAttachment` shape.
///
/// OpenCode v2 accepts `{ uri, name? }` for prompt attachments. Inline data must
/// be a `data:` URI; files on the node are referenced with a `file:` URI and are
/// read by the OpenCode server, not by this driver.
pub(super) fn attachments_to_prompt_files(
    attachments: &[fxg_protocol::common::AttachmentMeta],
) -> Vec<Value> {
    let mut files = Vec::new();
    for attachment in attachments {
        if let Some(data) = attachment.data_b64.as_deref() {
            files.push(json!({
                "uri": format!("data:{};base64,{data}", attachment.mime_type),
                "source": { "type": "inline" },
                "name": attachment.file_name,
            }));
            continue;
        }
        let Some(path) = attachment.local_path.as_deref() else {
            continue;
        };
        files.push(json!({
            "uri": file_uri(path),
            "name": attachment.file_name,
        }));
    }
    files
}

pub(super) fn file_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let encoded = percent_encode_file_path(&normalized);
    if normalized.starts_with("//") {
        // UNC share: file://server/share
        format!("file:{encoded}")
    } else if normalized.starts_with('/') {
        // Unix absolute path: file:///path
        format!("file://{encoded}")
    } else {
        // Windows drive path: file:///C:/path
        format!("file:///{encoded}")
    }
}

/// Percent-encodes a file path while keeping URI path separators and the
/// Windows drive colon intact.
fn percent_encode_file_path(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Deep-object query parameter used by OpenCode's project-scoped endpoints.
pub(super) fn location_query(cwd: &Path) -> String {
    format!(
        "location%5Bdirectory%5D={}",
        urlencoded(&cwd.to_string_lossy())
    )
}

pub(super) fn urlencoded(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_todowrite_input_to_plan_entries() {
        let input = json!({
            "todos": [
                {"content": "Write code", "status": "in_progress"},
                {"id": "todo-2", "content": "Run tests", "status": "completed"},
                {"content": "Ship", "status": "cancelled"}
            ]
        });
        let entries = todo_plan_entries("todowrite", &input).expect("plan");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "plan-0");
        assert_eq!(entries[0].title, "Write code");
        assert_eq!(entries[0].status, PlanEntryStatus::InProgress);
        assert_eq!(entries[1].id, "todo-2");
        assert_eq!(entries[1].status, PlanEntryStatus::Completed);
        assert_eq!(entries[2].status, PlanEntryStatus::Pending);
    }

    #[test]
    fn accepts_todowrite_input_encoded_as_a_json_string() {
        let input = json!({
            "todos": "[{\"content\":\"Write code\",\"status\":\"pending\"}]"
        });
        let entries = todo_plan_entries("todowrite", &input).expect("plan");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Write code");
    }

    #[test]
    fn an_empty_todo_list_clears_the_plan() {
        let entries = todo_plan_entries("todowrite", &json!({"todos": []})).expect("plan");
        assert!(entries.is_empty());
        assert!(todo_plan_entries("read", &json!({"todos": []})).is_none());
    }

    #[test]
    fn maps_finish_reasons_to_turn_stop_reasons() {
        assert_eq!(
            turn_stop_reason(Some("length"), None),
            Some(TurnStopReason::MaxTokens)
        );
        assert_eq!(
            turn_stop_reason(Some("content-filter"), None),
            Some(TurnStopReason::Refusal)
        );
        assert_eq!(
            turn_stop_reason(None, Some("provider.content-filter")),
            Some(TurnStopReason::Refusal)
        );
        assert_eq!(
            turn_stop_reason(Some("max-turns"), None),
            Some(TurnStopReason::MaxTurnRequests)
        );
        assert_eq!(turn_stop_reason(Some("stop"), None), None);
        assert_eq!(turn_stop_reason(None, Some("provider.auth")), None);
    }

    #[test]
    fn provider_auth_errors_carry_a_login_hint() {
        let message = error_message(
            &json!({"error": {"type": "provider.auth", "message": "no credential"}}),
            "fallback",
        );
        assert!(message.contains("opencode auth login"));
        assert!(message.contains("fxg agents login opencode"));
    }
}
