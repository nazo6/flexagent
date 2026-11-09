use super::terminal::*;
use super::*;

/// Turns ACP `StopReason` values that should be surfaced to the user into events.
///
/// `EndTurn` (normal completion) / `Cancelled` (user cancellation) count as
/// normal completion and return `None` (`StopReason` is `#[non_exhaustive]`).
pub(super) fn turn_ended_event(reason: StopReason) -> Option<UnifiedEventPayload> {
    let reason = match reason {
        StopReason::MaxTokens => TurnStopReason::MaxTokens,
        StopReason::MaxTurnRequests => TurnStopReason::MaxTurnRequests,
        StopReason::Refusal => TurnStopReason::Refusal,
        _ => return None,
    };
    Some(UnifiedEventPayload::TurnEnded { reason })
}

/// Extracts the session's modes, commands and config options.
pub(super) fn session_capabilities(
    modes: Option<&SessionModeState>,
    config_options: Option<&[SessionConfigOption]>,
) -> (Vec<ModeInfo>, Vec<CommandInfo>, Vec<ConfigOptionInfo>) {
    let modes = modes
        .map(|state| {
            state
                .available_modes
                .iter()
                .map(|mode| ModeInfo {
                    mode_id: mode.id.to_string(),
                    name: mode.name.clone(),
                    description: mode.description.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let config_options = config_options
        .map(|options| options.iter().map(map_config_option).collect())
        .unwrap_or_default();
    // Slash commands arrive via session/update (AvailableCommandsUpdate), so this
    // starts empty (later notifications update it)
    (modes, Vec::new(), config_options)
}

/// Builds ACP content blocks from a prompt and its attachments.
///
/// Images are sent as `ImageContent` when the agent advertises `image`
/// support. Non-image attachments use `ResourceLink`, which every agent must
/// support. When image support is unavailable, the attachment is described in
/// text so the user's input is not lost.
pub(super) fn prompt_blocks(
    input: PromptInput,
    capabilities: &PromptCapabilities,
) -> anyhow::Result<Vec<ContentBlock>> {
    let mut blocks = Vec::new();
    if !input.text.trim().is_empty() {
        blocks.push(ContentBlock::Text(TextContent::new(input.text)));
    }
    for attachment in input.attachments {
        if attachment.mime_type.starts_with("image/") {
            let data = match attachment.data_b64 {
                Some(data) => data,
                None => {
                    let path = attachment.local_path.as_deref().ok_or_else(|| {
                        anyhow!(
                            "image attachment {} has no data or local path",
                            attachment.file_name
                        )
                    })?;
                    base64_encode(
                        &std::fs::read(path)
                            .with_context(|| format!("failed to read image attachment {path}"))?,
                    )
                }
            };
            if capabilities.image {
                blocks.push(ContentBlock::Image(ImageContent::new(
                    data,
                    attachment.mime_type,
                )));
            } else {
                blocks.push(ContentBlock::Text(TextContent::new(format!(
                    "[image attachment: {} ({})]",
                    attachment.file_name,
                    attachment.local_path.as_deref().unwrap_or("inline")
                ))));
            }
        } else {
            let uri = attachment
                .local_path
                .as_deref()
                .map(file_uri)
                .unwrap_or_else(|| attachment.file_name.clone());
            blocks.push(ContentBlock::ResourceLink(
                ResourceLink::new(attachment.file_name, uri).mime_type(attachment.mime_type),
            ));
        }
    }
    if blocks.is_empty() {
        bail!("prompt has no content");
    }
    Ok(blocks)
}

/// Builds a `file://` URI for a node-local path.
pub(super) fn file_uri(path: &str) -> String {
    #[cfg(windows)]
    {
        format!("file:///{}", path.replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        format!("file://{path}")
    }
}

/// Converts an ACP authentication method into fxg's display / login type.
pub(crate) fn map_auth_method(method: &AuthMethod) -> AgentAuthMethodInfo {
    let id = method.id().to_string();
    let name = method.name().to_owned();
    let description = method.description().map(ToOwned::to_owned);
    match method {
        AuthMethod::Agent(_) => AgentAuthMethodInfo {
            id,
            name,
            description,
            kind: AgentAuthMethodKind::Agent,
            args: Vec::new(),
            env: BTreeMap::new(),
        },
        AuthMethod::Terminal(terminal) => AgentAuthMethodInfo {
            id,
            name,
            description,
            kind: AgentAuthMethodKind::Terminal,
            args: terminal.args.clone(),
            env: terminal
                .env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        },
        _ => AgentAuthMethodInfo {
            id,
            name,
            description,
            kind: AgentAuthMethodKind::Unknown("unknown".to_owned()),
            args: Vec::new(),
            env: BTreeMap::new(),
        },
    }
}

/// Converts an ACP config option into FXG's display type.
pub(super) fn map_config_option(option: &SessionConfigOption) -> ConfigOptionInfo {
    let (current_value, options) = match &option.kind {
        SessionConfigKind::Select(select) => (
            ConfigOptionValue::Select(select.current_value.to_string()),
            match &select.options {
                agent_client_protocol::v1::schema::SessionConfigSelectOptions::Ungrouped(
                    values,
                ) => values
                    .iter()
                    .map(|value| ConfigOptionValue::Select(value.value.to_string()))
                    .collect(),
                agent_client_protocol::v1::schema::SessionConfigSelectOptions::Grouped(groups) => {
                    groups
                        .iter()
                        .flat_map(|group| &group.options)
                        .map(|value| ConfigOptionValue::Select(value.value.to_string()))
                        .collect()
                }
                _ => Vec::new(),
            },
        ),
        SessionConfigKind::Boolean(boolean) => (
            ConfigOptionValue::Boolean(boolean.current_value),
            vec![
                ConfigOptionValue::Boolean(true),
                ConfigOptionValue::Boolean(false),
            ],
        ),
        _ => (
            ConfigOptionValue::Unknown(serde_json::Value::Null.into()),
            Vec::new(),
        ),
    };
    ConfigOptionInfo {
        key: option.id.to_string(),
        name: option.name.clone(),
        current_value,
        options,
    }
}

/// Converts an FXG config value into the ACP wire value.
///
/// FXG exposes select values as bare strings (for example
/// `"opencode-go/deepseek-v4.1-flash"`), while ACP serializes the same value as
/// `{ "value": "..." }`. Deserializing the bare string directly into
/// [`SessionConfigOptionValue`] fails, so map the scalar shapes explicitly and
/// only fall back to serde for the tagged (object) forms.
pub(super) fn parse_config_value(
    value: ConfigOptionValue,
) -> anyhow::Result<SessionConfigOptionValue> {
    match value {
        ConfigOptionValue::Select(id) => Ok(SessionConfigOptionValue::value_id(id)),
        ConfigOptionValue::Boolean(enabled) => Ok(SessionConfigOptionValue::boolean(enabled)),
        ConfigOptionValue::Unknown(raw) => {
            serde_json::from_value(raw.into_inner()).map_err(Into::into)
        }
    }
}

/// Tracking state for session capabilities (modes, commands, config options).
#[derive(Debug, Default, Clone)]
pub(super) struct SessionCapabilitiesState {
    pub(super) current_mode: Option<String>,
    pub(super) available_modes: Vec<ModeInfo>,
    pub(super) available_commands: Vec<CommandInfo>,
    pub(super) config_options: Vec<ConfigOptionInfo>,
    pub(super) fork_supported: Option<bool>,
}

impl SessionCapabilitiesState {
    pub(super) fn to_payload(&self) -> UnifiedEventPayload {
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: self.current_mode.clone(),
            available_modes: self.available_modes.clone(),
            available_commands: self.available_commands.clone(),
            config_options: self.config_options.clone(),
            fork_supported: self.fork_supported,
        }
    }
}

/// Accumulates streaming chunks and emits completion events when the stream changes.
///
/// Only completion events are persisted (docs/01 §2.3), and they consume a
/// `node_seq` at the moment they are emitted. A stream must therefore be
/// finalized as soon as the agent moves on to a different stream, otherwise the
/// reasoning would be recorded after the answer it preceded.
#[derive(Debug, Default)]
pub(super) struct StreamingAccumulator {
    pub(super) message: Option<(String, String)>,
    pub(super) thought: Option<(String, String)>,
}

impl StreamingAccumulator {
    pub(super) fn reset(&mut self) {
        self.message = None;
        self.thought = None;
    }

    pub(super) fn finish_message(&mut self) -> Option<UnifiedEventPayload> {
        self.message
            .take()
            .map(|(message_id, text)| UnifiedEventPayload::AgentMessage {
                message_id,
                text,
                is_complete: true,
            })
    }

    pub(super) fn finish_thought(&mut self) -> Option<UnifiedEventPayload> {
        self.thought
            .take()
            .map(|(thought_id, text)| UnifiedEventPayload::AgentThought {
                thought_id,
                text,
                is_complete: true,
            })
    }

    /// Appends a message chunk. Returns the completion events for any stream that
    /// ended before this message started (a thought, or a message with another ID).
    pub(super) fn push_message(
        &mut self,
        message_id: String,
        delta: &str,
    ) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        // Reasoning always precedes the answer it belongs to.
        out.extend(self.finish_thought());
        if self
            .message
            .as_ref()
            .is_some_and(|(id, _)| *id != message_id)
        {
            out.extend(self.finish_message());
        }
        match &mut self.message {
            Some((_, text)) => text.push_str(delta),
            None => self.message = Some((message_id, delta.to_owned())),
        }
        out
    }

    /// Appends a thought chunk. Returns the completion events for any stream that
    /// ended before this thought started (a message, or a thought with another ID).
    pub(super) fn push_thought(
        &mut self,
        thought_id: String,
        delta: &str,
    ) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        out.extend(self.finish_message());
        if self
            .thought
            .as_ref()
            .is_some_and(|(id, _)| *id != thought_id)
        {
            out.extend(self.finish_thought());
        }
        match &mut self.thought {
            Some((_, text)) => text.push_str(delta),
            None => self.thought = Some((thought_id, delta.to_owned())),
        }
        out
    }

    /// Takes the accumulated message / thought as completion events.
    pub(super) fn finish_all(&mut self) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        // Defensive ordering: reasoning precedes the answer within a turn.
        out.extend(self.finish_thought());
        out.extend(self.finish_message());
        out
    }
}

/// Converts a `session/update` notification into a list of [`DriverEvent`]s.
pub(super) fn map_session_update(
    update: SessionUpdate,
    accumulator: &mut StreamingAccumulator,
    capabilities: &mut SessionCapabilitiesState,
) -> Vec<DriverEvent> {
    let mut events = Vec::new();
    match update {
        SessionUpdate::UserMessageChunk(_) => {
            // User messages are already recorded by the manager as `UserMessage`
        }
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content,
            message_id,
            ..
        }) => {
            if let ContentBlock::Text(TextContent { text, .. }) = content {
                let message_id = message_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "acp-message".to_owned());
                for finished in accumulator.push_message(message_id.clone(), &text) {
                    events.push(DriverEvent::Event(finished));
                }
                events.push(DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta {
                    message_id,
                    text_delta: text,
                }));
            }
        }
        SessionUpdate::AgentThoughtChunk(ContentChunk {
            content,
            message_id,
            ..
        }) => {
            if let ContentBlock::Text(TextContent { text, .. }) = content {
                let thought_id = message_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "acp-thought".to_owned());
                for finished in accumulator.push_thought(thought_id.clone(), &text) {
                    events.push(DriverEvent::Event(finished));
                }
                events.push(DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta {
                    thought_id,
                    text_delta: text,
                }));
            }
        }
        SessionUpdate::ToolCall(tool_call) => {
            events.push(DriverEvent::Event(tool_call_event(&tool_call)));
        }
        SessionUpdate::ToolCallUpdate(update) => {
            events.push(DriverEvent::Event(tool_call_update_event(&update)));
        }
        SessionUpdate::Plan(plan) => {
            let entries = plan
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| PlanEntry {
                    id: format!("plan-{index}"),
                    title: entry.content.clone(),
                    status: plan_status(&entry.status),
                })
                .collect();
            events.push(DriverEvent::Event(UnifiedEventPayload::PlanUpdate {
                entries,
            }));
        }
        SessionUpdate::AvailableCommandsUpdate(update) => {
            let commands = update
                .available_commands
                .iter()
                .map(|command| CommandInfo {
                    name: command.name.clone(),
                    description: command.description.clone(),
                    input_hint: command.input.as_ref().map(|input| match input {
                        agent_client_protocol::v1::schema::AvailableCommandInput::Unstructured(
                            unstructured,
                        ) => unstructured.hint.clone(),
                        _ => String::new(),
                    }),
                })
                .collect();
            capabilities.available_commands = commands;
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::CurrentModeUpdate(update) => {
            capabilities.current_mode = Some(update.current_mode_id.to_string());
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::ConfigOptionUpdate(update) => {
            capabilities.config_options = update
                .config_options
                .iter()
                .map(map_config_option)
                .collect();
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::SessionInfoUpdate(update) => {
            if let Some(title) = update.title.value() {
                let trimmed = title.trim();
                if !trimmed.is_empty() {
                    events.push(DriverEvent::Event(
                        UnifiedEventPayload::SessionTitleChanged {
                            title: trimmed.to_owned(),
                        },
                    ));
                }
            }
        }
        SessionUpdate::UsageUpdate(update) => {
            events.push(DriverEvent::Event(UnifiedEventPayload::UsageUpdated {
                used_tokens: update.used,
                context_size: update.size,
                cost: update.cost.map(|cost| UsageCost {
                    amount: cost.amount,
                    currency: cost.currency,
                }),
            }));
        }
        // `SessionUpdate` is #[non_exhaustive]
        other => {
            tracing::trace!("unhandled session update: {other:?}");
        }
    }
    events
}

/// Converts an ACP `ToolCall` into a normalized event.
pub(super) fn tool_call_event(tool_call: &ToolCall) -> UnifiedEventPayload {
    let (diff, _, text_content) = tool_call_contents(&tool_call.content);
    UnifiedEventPayload::ToolCall {
        tool_call_id: tool_call.tool_call_id.to_string(),
        title: tool_call.title.clone(),
        kind: tool_kind(tool_call.kind),
        status: tool_status(tool_call.status),
        locations: tool_call
            .locations
            .iter()
            .map(|location| location.path.to_string_lossy().into_owned())
            .collect(),
        diff,
        raw_output: tool_call
            .raw_output
            .as_ref()
            .map(|value| value.to_string())
            .or_else(|| (!text_content.is_empty()).then(|| text_content.join("\n"))),
    }
}

/// Converts an ACP `ToolCallUpdate` into a normalized event (appended to the same `tool_call_id`).
pub(super) fn tool_call_update_event(update: &ToolCallUpdate) -> UnifiedEventPayload {
    let fields = &update.fields;
    let (diff, locations, text_content) = fields
        .content
        .as_ref()
        .map(|content| tool_call_contents(content))
        .unwrap_or((None, Vec::new(), Vec::new()));
    let locations = fields
        .locations
        .as_ref()
        .map(|values| {
            values
                .iter()
                .map(|location| location.path.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or(locations);
    UnifiedEventPayload::ToolCall {
        tool_call_id: update.tool_call_id.to_string(),
        title: fields.title.clone().unwrap_or_default(),
        kind: fields
            .kind
            .map(tool_kind)
            .unwrap_or_else(|| FxgToolCallKind::Unknown(String::new())),
        status: fields
            .status
            .map(tool_status)
            .unwrap_or_else(|| FxgToolCallStatus::Unknown(String::new())),
        locations,
        diff,
        raw_output: fields
            .raw_output
            .as_ref()
            .map(|value| value.to_string())
            .or_else(|| (!text_content.is_empty()).then(|| text_content.join("\n"))),
    }
}

/// Extracts the diff, file paths and text output from a `ToolCallContent` list.
pub(super) fn tool_call_contents(
    contents: &[ToolCallContent],
) -> (Option<FileDiff>, Vec<String>, Vec<String>) {
    let mut diff = None;
    let mut locations = Vec::new();
    let mut texts = Vec::new();
    for content in contents {
        match content {
            ToolCallContent::Diff(acp_diff) => {
                let path = acp_diff.path.to_string_lossy().into_owned();
                locations.push(path.clone());
                let old_text = acp_diff.old_text.clone().unwrap_or_default();
                diff = Some(build_file_diff(&path, &old_text, &acp_diff.new_text));
            }
            ToolCallContent::Content(content) => {
                if let ContentBlock::Text(text) = &content.content {
                    texts.push(text.text.clone());
                }
            }
            _ => {}
        }
    }
    (diff, locations, texts)
}

/// Computes a unified diff from the before/after text and builds a [`FileDiff`].
pub(crate) fn build_file_diff(path: &str, old_text: &str, new_text: &str) -> FileDiff {
    let patch = diffy::create_patch(old_text, new_text);
    let unified_diff = patch.to_string();
    let mut additions = 0u32;
    let mut deletions = 0u32;
    for line in unified_diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            additions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }
    FileDiff {
        path: path.to_owned(),
        old_text: Some(old_text.to_owned()),
        new_text: Some(new_text.to_owned()),
        unified_diff,
        additions,
        deletions,
    }
}

/// Serializes an ACP enum value to the snake_case string used on the wire.
pub(super) fn acp_wire_value<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        _ => "unknown".to_owned(),
    }
}

pub(super) fn tool_kind(kind: ToolKind) -> FxgToolCallKind {
    match kind {
        ToolKind::Read => FxgToolCallKind::Read,
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move => FxgToolCallKind::Edit,
        ToolKind::Search => FxgToolCallKind::Search,
        ToolKind::Execute => FxgToolCallKind::Execute,
        ToolKind::Think | ToolKind::Fetch | ToolKind::SwitchMode | ToolKind::Other => {
            FxgToolCallKind::Other
        }
        _ => FxgToolCallKind::Unknown(acp_wire_value(&kind)),
    }
}

pub(super) fn tool_status(status: ToolCallStatus) -> FxgToolCallStatus {
    match status {
        ToolCallStatus::Pending => FxgToolCallStatus::Pending,
        ToolCallStatus::InProgress => FxgToolCallStatus::InProgress,
        ToolCallStatus::Completed => FxgToolCallStatus::Completed,
        ToolCallStatus::Failed => FxgToolCallStatus::Failed,
        _ => FxgToolCallStatus::Unknown(acp_wire_value(&status)),
    }
}

pub(super) fn plan_status(status: &AcpPlanEntryStatus) -> FxgPlanEntryStatus {
    match status {
        AcpPlanEntryStatus::Pending => FxgPlanEntryStatus::Pending,
        AcpPlanEntryStatus::InProgress => FxgPlanEntryStatus::InProgress,
        AcpPlanEntryStatus::Completed => FxgPlanEntryStatus::Completed,
        _ => FxgPlanEntryStatus::Unknown(acp_wire_value(status)),
    }
}

pub(super) fn permission_kind(kind: AcpPermissionOptionKind) -> FxgPermissionOptionKind {
    match kind {
        AcpPermissionOptionKind::AllowOnce => FxgPermissionOptionKind::AllowOnce,
        AcpPermissionOptionKind::AllowAlways => FxgPermissionOptionKind::AllowAlways,
        AcpPermissionOptionKind::RejectOnce => FxgPermissionOptionKind::RejectOnce,
        AcpPermissionOptionKind::RejectAlways => FxgPermissionOptionKind::RejectAlways,
        _ => FxgPermissionOptionKind::Unknown(acp_wire_value(&kind)),
    }
}
