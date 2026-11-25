//! History replay helpers for the native OpenCode driver.

use super::*;

pub(super) fn initial_model(
    initial_config: &[fxg_protocol::common::InitialConfig],
) -> Option<String> {
    initial_config.iter().find_map(|config| {
        if config.key == "model" {
            config.value.select_value().map(str::to_owned)
        } else {
            None
        }
    })
}

pub(super) fn emit_history(runtime: &SessionRuntime, messages: &[Value]) {
    for message in messages {
        let kind = message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "user" => {
                let text = message
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let attachments = attachments_from_prompt_files(message.get("files"));
                if text.is_empty() && attachments.is_empty() {
                    continue;
                }
                runtime.emit_event(UnifiedEventPayload::UserMessage {
                    text,
                    attachments,
                    client_source: ClientSource::Import,
                    agent_message_id: (!message_id.is_empty()).then(|| message_id.to_owned()),
                });
            }
            "assistant" => {
                let Some(content) = message.get("content").and_then(Value::as_array) else {
                    continue;
                };
                for (index, part) in content.iter().enumerate() {
                    match part.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                runtime.emit_event(UnifiedEventPayload::AgentMessage {
                                    message_id: format!("{message_id}:text:{index}"),
                                    text: text.to_owned(),
                                    attachments: Vec::new(),
                                    is_complete: true,
                                    subagent_id: None,
                                });
                            }
                        }
                        Some("reasoning") => {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                runtime.emit_event(UnifiedEventPayload::AgentThought {
                                    thought_id: format!("{message_id}:reasoning:{index}"),
                                    text: text.to_owned(),
                                    attachments: Vec::new(),
                                    is_complete: true,
                                    subagent_id: None,
                                });
                            }
                        }
                        Some("tool") => {
                            let name = part
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_owned();
                            let state = part.get("state").cloned().unwrap_or(Value::Null);
                            let status = match state.get("status").and_then(Value::as_str) {
                                Some("completed") => ToolCallStatus::Completed,
                                Some("error") | Some("failed") => ToolCallStatus::Failed,
                                Some("running") => ToolCallStatus::InProgress,
                                _ => ToolCallStatus::Pending,
                            };
                            let input = state.get("input").cloned().unwrap_or(Value::Null);
                            let mut raw_output = tool_output(&state);
                            if status == ToolCallStatus::Failed
                                && let Some(error) =
                                    state.pointer("/error/message").and_then(Value::as_str)
                            {
                                raw_output = Some(match raw_output {
                                    Some(output) => format!("{output}\n{error}"),
                                    None => error.to_owned(),
                                });
                            }
                            runtime.emit_event(UnifiedEventPayload::ToolCall {
                                tool_call_id: part
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                title: tool_title(&name, &input),
                                kind: tool_kind(&name),
                                status,
                                locations: tool_locations(&input),
                                diff: tool_diff(&name, &input),
                                raw_output,
                                terminal_id: None,
                                subagent_id: None,
                            });
                            if let Some(entries) = todo_plan_entries(&name, &input) {
                                runtime.emit_event(UnifiedEventPayload::PlanUpdate {
                                    entries,
                                    subagent_id: None,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}
