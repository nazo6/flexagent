use std::collections::BTreeMap;

use super::terminal::*;
use super::*;
use agent_client_protocol::v1::schema::{SessionConfigSelectOption, SessionConfigSelectOptions};

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

/// Extracts the session's mode state and config options.
///
/// ACP supersedes Session Modes with Session Config Options, and agents such as
/// OpenCode only report the selector as `configOptions[category = mode]`, so the
/// config option takes precedence over the legacy `modes` field.
pub(super) fn session_capabilities(
    modes: Option<&SessionModeState>,
    config_options: Option<&[SessionConfigOption]>,
) -> (Vec<ModeInfo>, Option<String>, Vec<ConfigOptionInfo>) {
    let (available_modes, current_mode) = mode_config_option(config_options.unwrap_or_default())
        .and_then(mode_state_from_config)
        .unwrap_or_else(|| {
            (
                modes
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
                    .unwrap_or_default(),
                modes.map(|state| state.current_mode_id.to_string()),
            )
        });
    let config_options = config_options
        .map(|options| options.iter().map(map_config_option).collect())
        .unwrap_or_default();
    // Slash commands arrive via session/update (AvailableCommandsUpdate), so they
    // are not part of the session/new payload.
    (available_modes, current_mode, config_options)
}

/// The config option that carries the session mode, if the agent reports one.
///
/// Categories are UX hints (ACP: clients MUST handle missing or unknown
/// categories), so an uncategorized option named `mode` is accepted as well,
/// matching the Web UI.
pub(super) fn mode_config_option(options: &[SessionConfigOption]) -> Option<&SessionConfigOption> {
    options.iter().find(|option| {
        option.category.as_ref() == Some(&SessionConfigOptionCategory::Mode)
            || (option.category.is_none() && option.id.to_string() == "mode")
    })
}

/// Projects a mode selector config option into FXG's mode list / current mode.
fn mode_state_from_config(option: &SessionConfigOption) -> Option<(Vec<ModeInfo>, Option<String>)> {
    let SessionConfigKind::Select(select) = &option.kind else {
        return None;
    };
    let to_mode = |value: &SessionConfigSelectOption| ModeInfo {
        mode_id: value.value.to_string(),
        name: value.name.clone(),
        description: value.description.clone(),
    };
    let mut modes = Vec::new();
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(values) => {
            modes.extend(values.iter().map(to_mode));
        }
        SessionConfigSelectOptions::Grouped(groups) => {
            modes.extend(groups.iter().flat_map(|group| &group.options).map(to_mode));
        }
        // `SessionConfigSelectOptions` is non-exhaustive
        _ => {}
    }
    if modes.is_empty() {
        return None;
    }
    Some((modes, Some(select.current_value.to_string())))
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
        description: option.description.clone(),
        category: option.category.as_ref().map(map_config_category),
        current_value,
        options,
    }
}

/// Maps the ACP config option category to FXG's open enum.
fn map_config_category(category: &SessionConfigOptionCategory) -> ConfigOptionCategory {
    match category {
        SessionConfigOptionCategory::Mode => ConfigOptionCategory::Mode,
        SessionConfigOptionCategory::Model => ConfigOptionCategory::Model,
        SessionConfigOptionCategory::ModelConfig => ConfigOptionCategory::ModelConfig,
        SessionConfigOptionCategory::ThoughtLevel => ConfigOptionCategory::ThoughtLevel,
        SessionConfigOptionCategory::Other(raw) => ConfigOptionCategory::Unknown(raw.clone()),
        // `SessionConfigOptionCategory` is non-exhaustive; preserve future values as-is.
        _ => ConfigOptionCategory::Unknown(String::new()),
    }
}

/// Converts an FXG config value into the ACP wire value.
///
/// FXG exposes select values as bare strings (for example
/// `"provider/model"`), while ACP serializes the same value as
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
    pub(super) fork_scope: Option<ForkScope>,
}

impl SessionCapabilitiesState {
    pub(super) fn to_payload(&self) -> UnifiedEventPayload {
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: self.current_mode.clone(),
            available_modes: self.available_modes.clone(),
            available_commands: self.available_commands.clone(),
            config_options: self.config_options.clone(),
            fork_scope: self.fork_scope,
        }
    }

    /// Replaces the config options, keeping the mode projection in sync.
    ///
    /// Agents that expose modes as a config option (ACP's current model) own the
    /// mode list and current mode, so `config_option_update` must refresh both.
    pub(super) fn set_config_options(&mut self, options: &[SessionConfigOption]) {
        self.config_options = options.iter().map(map_config_option).collect();
        let Some((modes, current_mode)) =
            mode_config_option(options).and_then(mode_state_from_config)
        else {
            return;
        };
        self.available_modes = modes;
        if current_mode.is_some() {
            self.current_mode = current_mode;
        }
    }
}

/// Whether the option is the session mode selector (the ACP category, or the
/// conventional `mode` key when the agent omits categories).
pub(super) fn is_mode_option(option: &ConfigOptionInfo) -> bool {
    option.category.as_ref() == Some(&ConfigOptionCategory::Mode)
        || (option.category.is_none() && option.key == "mode")
}

/// Where a `session/update` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpdateMode {
    /// A live turn / background update (user messages are recorded by the manager).
    Live,
    /// History replayed by `session/load` for an explicit import (user messages
    /// come from the replay and must be recorded).
    Import,
}

/// Accumulates streaming chunks per session scope (parent or subagent).
///
/// Only completion events are persisted (docs/01 §2.3), and they consume a
/// `node_seq` at the moment they are emitted. A stream must therefore be
/// finalized as soon as the agent moves on to a different stream, otherwise the
/// reasoning would be recorded after the answer it preceded. The scope is part
/// of the key so concurrently running subagents do not finalize each other's
/// streams.
#[derive(Debug, Default)]
pub(super) struct StreamingAccumulator {
    pub(super) messages: BTreeMap<Option<String>, (String, String)>,
    pub(super) thoughts: BTreeMap<Option<String>, (String, String)>,
    /// Replayed user messages (import only; empty during live turns).
    pub(super) user_messages: BTreeMap<Option<String>, (String, String)>,
    /// Sender / recipient of a session message, keyed by message ID (used to
    /// attach direction to chunks that arrive after the upsert).
    pub(super) session_messages: BTreeMap<String, (String, String)>,
}

impl StreamingAccumulator {
    pub(super) fn reset(&mut self) {
        self.messages.clear();
        self.thoughts.clear();
        self.user_messages.clear();
        self.session_messages.clear();
    }

    pub(super) fn finish_user_message(
        &mut self,
        subagent_id: &Option<String>,
    ) -> Option<UnifiedEventPayload> {
        self.user_messages
            .remove(subagent_id)
            .map(|(_message_id, text)| UnifiedEventPayload::UserMessage {
                text,
                attachments: Vec::new(),
                client_source: fxg_protocol::common::ClientSource::Import,
            })
    }

    pub(super) fn finish_message(
        &mut self,
        subagent_id: &Option<String>,
    ) -> Option<UnifiedEventPayload> {
        self.messages.remove(subagent_id).map(|(message_id, text)| {
            UnifiedEventPayload::AgentMessage {
                message_id,
                text,
                is_complete: true,
                subagent_id: subagent_id.clone(),
            }
        })
    }

    pub(super) fn finish_thought(
        &mut self,
        subagent_id: &Option<String>,
    ) -> Option<UnifiedEventPayload> {
        self.thoughts.remove(subagent_id).map(|(thought_id, text)| {
            UnifiedEventPayload::AgentThought {
                thought_id,
                text,
                is_complete: true,
                subagent_id: subagent_id.clone(),
            }
        })
    }

    /// Appends a message chunk. Returns completion events for streams that
    /// ended before this message started within the same scope.
    pub(super) fn push_message(
        &mut self,
        subagent_id: Option<String>,
        message_id: String,
        delta: &str,
    ) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        // A replayed user message precedes the answer it belongs to.
        out.extend(self.finish_user_message(&subagent_id));
        // Reasoning always precedes the answer it belongs to.
        out.extend(self.finish_thought(&subagent_id));
        if self
            .messages
            .get(&subagent_id)
            .is_some_and(|(id, _)| *id != message_id)
        {
            out.extend(self.finish_message(&subagent_id));
        }
        match self.messages.get_mut(&subagent_id) {
            Some((_, text)) => text.push_str(delta),
            None => {
                self.messages
                    .insert(subagent_id, (message_id, delta.to_owned()));
            }
        }
        out
    }

    /// Appends a thought chunk. Returns completion events for streams that
    /// ended before this thought started within the same scope.
    pub(super) fn push_thought(
        &mut self,
        subagent_id: Option<String>,
        thought_id: String,
        delta: &str,
    ) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        out.extend(self.finish_user_message(&subagent_id));
        out.extend(self.finish_message(&subagent_id));
        if self
            .thoughts
            .get(&subagent_id)
            .is_some_and(|(id, _)| *id != thought_id)
        {
            out.extend(self.finish_thought(&subagent_id));
        }
        match self.thoughts.get_mut(&subagent_id) {
            Some((_, text)) => text.push_str(delta),
            None => {
                self.thoughts
                    .insert(subagent_id, (thought_id, delta.to_owned()));
            }
        }
        out
    }

    /// Appends a replayed user message chunk (import only).
    ///
    /// Returns completion events for the previous turn's answer / reasoning and
    /// for a previous user message with a different ID.
    pub(super) fn push_user_message(
        &mut self,
        subagent_id: Option<String>,
        message_id: String,
        delta: &str,
    ) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        out.extend(self.finish_thought(&subagent_id));
        out.extend(self.finish_message(&subagent_id));
        if self
            .user_messages
            .get(&subagent_id)
            .is_some_and(|(id, _)| *id != message_id)
        {
            out.extend(self.finish_user_message(&subagent_id));
        }
        match self.user_messages.get_mut(&subagent_id) {
            Some((_, text)) => text.push_str(delta),
            None => {
                self.user_messages
                    .insert(subagent_id, (message_id, delta.to_owned()));
            }
        }
        out
    }

    /// Takes every accumulated stream as completion events.
    pub(super) fn finish_all(&mut self) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        let scopes: Vec<Option<String>> = self.user_messages.keys().cloned().collect();
        for scope in scopes {
            out.extend(self.finish_user_message(&scope));
        }
        // Defensive ordering: reasoning precedes the answer within a turn.
        let scopes: Vec<Option<String>> = self.thoughts.keys().cloned().collect();
        for scope in scopes {
            out.extend(self.finish_thought(&scope));
        }
        let scopes: Vec<Option<String>> = self.messages.keys().cloned().collect();
        for scope in scopes {
            out.extend(self.finish_message(&scope));
        }
        out
    }
}

/// Converts a live `session/update` notification into a list of [`DriverEvent`]s.
///
/// `subagent_id` is `None` for the root session and `Some(child)` for a child
/// session. Child-originated events are tagged, and capability updates from a
/// child never overwrite the parent's capabilities.
#[cfg(test)]
pub(super) fn map_session_update(
    update: SessionUpdate,
    subagent_id: Option<&str>,
    accumulator: &mut StreamingAccumulator,
    capabilities: &mut SessionCapabilitiesState,
) -> Vec<DriverEvent> {
    map_session_update_with_mode(
        update,
        subagent_id,
        accumulator,
        capabilities,
        UpdateMode::Live,
    )
}

/// Converts a `session/update` notification with an explicit source mode.
pub(super) fn map_session_update_with_mode(
    update: SessionUpdate,
    subagent_id: Option<&str>,
    accumulator: &mut StreamingAccumulator,
    capabilities: &mut SessionCapabilitiesState,
    mode: UpdateMode,
) -> Vec<DriverEvent> {
    let scope = subagent_id.map(str::to_owned);
    let mut events = Vec::new();
    match update {
        SessionUpdate::UserMessageChunk(ContentChunk {
            content,
            message_id,
            ..
        }) => {
            if mode == UpdateMode::Import {
                if let ContentBlock::Text(TextContent { text, .. }) = content {
                    let message_id = message_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "acp-user-message".to_owned());
                    for finished in accumulator.push_user_message(scope.clone(), message_id, &text)
                    {
                        events.push(DriverEvent::Event(finished));
                    }
                }
            } else {
                // User messages are already recorded by the manager as `UserMessage`.
                // The echo marks the start of a turn: a background report that arrived
                // before it (agy's teamwork only flushes its follow-up when the next
                // prompt starts) belongs to the previous context, so finalize it as its
                // own message instead of merging it into this turn's answer.
                let mut finished = Vec::new();
                if let Some(payload) = accumulator.finish_thought(&scope) {
                    finished.push(payload);
                }
                if let Some(payload) = accumulator.finish_message(&scope) {
                    finished.push(payload);
                }
                for payload in finished {
                    events.push(DriverEvent::Event(payload));
                }
            }
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
                for finished in accumulator.push_message(scope.clone(), message_id.clone(), &text) {
                    events.push(DriverEvent::Event(finished));
                }
                events.push(DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta {
                    message_id,
                    text_delta: text,
                    subagent_id: scope,
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
                for finished in accumulator.push_thought(scope.clone(), thought_id.clone(), &text) {
                    events.push(DriverEvent::Event(finished));
                }
                events.push(DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta {
                    thought_id,
                    text_delta: text,
                    subagent_id: scope,
                }));
            }
        }
        SessionUpdate::ToolCall(tool_call) => {
            events.push(DriverEvent::Event(tool_call_event(&tool_call, subagent_id)));
        }
        SessionUpdate::ToolCallUpdate(update) => {
            events.push(DriverEvent::Event(tool_call_update_event(
                &update,
                subagent_id,
            )));
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
                subagent_id: subagent_id.map(str::to_owned),
            }));
        }
        SessionUpdate::AvailableCommandsUpdate(update) => {
            if subagent_id.is_some() {
                tracing::trace!("ignoring child available_commands_update");
                return events;
            }
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
            if subagent_id.is_some() {
                tracing::trace!("ignoring child current_mode_update");
                return events;
            }
            capabilities.current_mode = Some(update.current_mode_id.to_string());
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::ConfigOptionUpdate(update) => {
            if subagent_id.is_some() {
                tracing::trace!("ignoring child config_option_update");
                return events;
            }
            capabilities.set_config_options(&update.config_options);
            events.push(DriverEvent::Event(capabilities.to_payload()));
        }
        SessionUpdate::SessionInfoUpdate(update) => {
            // A child's title must not rename the parent session.
            if subagent_id.is_some() {
                tracing::trace!("ignoring child session_info_update");
                return events;
            }
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
                subagent_id: subagent_id.map(str::to_owned),
            }));
        }
        SessionUpdate::SubagentUpdate(_) => {
            // Announcements need the root/child state and are handled by the
            // session loop (see `ChildSessions`).
        }
        SessionUpdate::SessionMessage(message) => {
            let sender = message
                .sender_session_id
                .value()
                .map(ToString::to_string)
                .unwrap_or_default();
            let recipient = message
                .recipient_session_id
                .value()
                .map(ToString::to_string)
                .unwrap_or_default();
            let message_id = message.message_id.to_string();
            let text = message
                .content
                .value()
                .map(|content| content_text(content))
                .unwrap_or_default();
            if !text.is_empty() {
                accumulator
                    .session_messages
                    .insert(message_id.clone(), (sender.clone(), recipient.clone()));
                events.push(DriverEvent::Event(UnifiedEventPayload::SubagentMessage {
                    message_id,
                    sender_session_id: sender,
                    recipient_session_id: recipient,
                    text,
                    is_complete: true,
                }));
            }
        }
        SessionUpdate::SessionMessageChunk(chunk) => {
            let message_id = chunk.message_id.to_string();
            let (sender, recipient) = accumulator
                .session_messages
                .get(&message_id)
                .cloned()
                .unwrap_or_default();
            if let ContentBlock::Text(TextContent { text, .. }) = chunk.content {
                events.push(DriverEvent::Delta(
                    StreamDeltaPayload::SubagentMessageDelta {
                        message_id,
                        sender_session_id: sender,
                        recipient_session_id: recipient,
                        text_delta: text,
                    },
                ));
            }
        }
        // `SessionUpdate` is #[non_exhaustive]
        other => {
            tracing::trace!("unhandled session update: {other:?}");
        }
    }
    events
}

/// Joins the text blocks of an inter-session message.
fn content_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(TextContent { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Converts an ACP `ToolCall` into a normalized event.
pub(super) fn tool_call_event(
    tool_call: &ToolCall,
    subagent_id: Option<&str>,
) -> UnifiedEventPayload {
    let (diff, _, text_content) = tool_call_contents(&tool_call.content);
    UnifiedEventPayload::ToolCall {
        subagent_id: subagent_id.map(str::to_owned),
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
pub(super) fn tool_call_update_event(
    update: &ToolCallUpdate,
    subagent_id: Option<&str>,
) -> UnifiedEventPayload {
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
        subagent_id: subagent_id.map(str::to_owned),
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
