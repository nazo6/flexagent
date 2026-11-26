use std::collections::BTreeMap;

use super::terminal::*;
use super::*;
use agent_client_protocol::v1::schema::{SessionConfigSelectOption, SessionConfigSelectOptions};
use fxg_protocol::common::AttachmentMeta;

use crate::file_uri;

mod types;

pub(crate) use types::*;

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
                .map(file_uri::to_uri)
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

/// Largest image / audio / blob payload persisted inline in an event.
///
/// ACP agents can emit arbitrarily large screenshots. Keeping them small avoids
/// bloating `node.db` and the outbox; larger blocks are still listed, but only
/// their metadata survives.
const MAX_INLINE_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;

/// A renderable piece of an ACP `ContentBlock`.
enum ChunkPart {
    Text(String),
    Attachment(AttachmentMeta),
}

/// Splits a content block into text or an attachment.
///
/// Text resources are folded into the body text; images, audio, resource links
/// and embedded blobs become attachments so the UI can render them without
/// losing the payload.
fn chunk_part(block: &ContentBlock) -> Option<ChunkPart> {
    match block {
        ContentBlock::Text(text) => Some(ChunkPart::Text(text.text.clone())),
        ContentBlock::Image(image) => Some(ChunkPart::Attachment(inline_attachment(
            "image",
            &image.mime_type,
            &image.data,
            image.uri.as_deref(),
        ))),
        ContentBlock::Audio(audio) => Some(ChunkPart::Attachment(inline_attachment(
            "audio",
            &audio.mime_type,
            &audio.data,
            None,
        ))),
        ContentBlock::ResourceLink(link) => Some(ChunkPart::Attachment(AttachmentMeta {
            file_name: if link.name.is_empty() {
                file_uri::file_name(&link.uri).unwrap_or_default()
            } else {
                link.name.clone()
            },
            mime_type: link
                .mime_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
            size_bytes: link.size.unwrap_or(0).max(0) as u64,
            local_path: file_uri::to_path(&link.uri),
            data_b64: None,
        })),
        ContentBlock::Resource(resource) => match &resource.resource {
            agent_client_protocol::v1::schema::EmbeddedResourceResource::TextResourceContents(
                text,
            ) => Some(ChunkPart::Text(text.text.clone())),
            agent_client_protocol::v1::schema::EmbeddedResourceResource::BlobResourceContents(
                blob,
            ) => Some(ChunkPart::Attachment(inline_attachment(
                "resource",
                blob.mime_type
                    .as_deref()
                    .unwrap_or("application/octet-stream"),
                &blob.blob,
                Some(&blob.uri),
            ))),
            _ => None,
        },
        _ => None,
    }
}

/// Builds attachment metadata for base64 inline content.
fn inline_attachment(
    default_name: &str,
    mime_type: &str,
    data: &str,
    uri: Option<&str>,
) -> AttachmentMeta {
    let size_bytes = base64_decoded_len(data);
    let file_name = uri
        .and_then(file_uri::file_name)
        .unwrap_or_else(|| format!("{default_name}.{}", extension_for_mime(mime_type)));
    let inline = size_bytes <= MAX_INLINE_ATTACHMENT_BYTES;
    if !inline {
        tracing::warn!(
            mime_type,
            size_bytes,
            "agent attachment exceeds the inline limit; keeping metadata only"
        );
    }
    AttachmentMeta {
        file_name,
        mime_type: mime_type.to_owned(),
        size_bytes,
        local_path: uri.and_then(file_uri::to_path),
        data_b64: inline.then(|| data.to_owned()),
    }
}

/// Size of standard base64 data without decoding it.
fn base64_decoded_len(data: &str) -> u64 {
    let padding = data.bytes().rev().take_while(|byte| *byte == b'=').count() as u64;
    ((data.len() as u64).saturating_mul(3) / 4).saturating_sub(padding)
}

/// File extension for the MIME types agents commonly emit.
fn extension_for_mime(mime_type: &str) -> &'static str {
    if mime_type == "application/octet-stream" {
        return "bin";
    }
    let mime_type = match mime_type {
        // A common legacy alias that mime_guess does not map back.
        "audio/x-wav" => "audio/wav",
        other => other,
    };
    let Some(extensions) = mime_guess::get_mime_extensions_str(mime_type) else {
        return "bin";
    };
    // mime_guess also lists uncommon aliases (jfif, m2a, oga, ...), so prefer
    // the extension users expect in a generated file name.
    const PREFERRED: &[&str] = &[
        "png", "jpg", "gif", "webp", "svg", "wav", "mp3", "ogg", "webm",
    ];
    for preferred in PREFERRED {
        if extensions.contains(preferred) {
            return preferred;
        }
    }
    extensions.first().copied().unwrap_or("bin")
}

/// Accumulates streaming chunks per session scope (parent or subagent).
///
/// Only completion events are persisted (docs/01 §2.3), and they consume a
/// `node_seq` at the moment they are emitted. A stream must therefore be
/// finalized as soon as the agent moves on to a different stream, otherwise the
/// reasoning would be recorded after the answer it preceded. The scope is part
/// of the key so concurrently running subagents do not finalize each other's
/// streams.
/// One stream (answer, thought or replayed user message) being accumulated.
#[derive(Debug, Default)]
pub(super) struct PendingStream {
    pub(super) id: String,
    pub(super) text: String,
    /// Non-text blocks that arrived inside the stream (images, resources, ...).
    pub(super) attachments: Vec<AttachmentMeta>,
}

#[derive(Debug, Default)]
pub(super) struct StreamingAccumulator {
    pub(super) messages: BTreeMap<Option<String>, PendingStream>,
    pub(super) thoughts: BTreeMap<Option<String>, PendingStream>,
    /// Replayed user messages (import only; empty during live turns).
    pub(super) user_messages: BTreeMap<Option<String>, PendingStream>,
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
            .map(|stream| UnifiedEventPayload::UserMessage {
                text: stream.text,
                attachments: stream.attachments,
                client_source: fxg_protocol::common::ClientSource::Import,
                agent_message_id: Some(stream.id),
            })
    }

    pub(super) fn finish_message(
        &mut self,
        subagent_id: &Option<String>,
    ) -> Option<UnifiedEventPayload> {
        self.messages
            .remove(subagent_id)
            .map(|stream| UnifiedEventPayload::AgentMessage {
                message_id: stream.id,
                text: stream.text,
                attachments: stream.attachments,
                is_complete: true,
                subagent_id: subagent_id.clone(),
            })
    }

    pub(super) fn finish_thought(
        &mut self,
        subagent_id: &Option<String>,
    ) -> Option<UnifiedEventPayload> {
        self.thoughts
            .remove(subagent_id)
            .map(|stream| UnifiedEventPayload::AgentThought {
                thought_id: stream.id,
                text: stream.text,
                attachments: stream.attachments,
                is_complete: true,
                subagent_id: subagent_id.clone(),
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
            .is_some_and(|stream| stream.id != message_id)
        {
            out.extend(self.finish_message(&subagent_id));
        }
        match self.messages.get_mut(&subagent_id) {
            Some(stream) => stream.text.push_str(delta),
            None => {
                self.messages.insert(
                    subagent_id,
                    PendingStream {
                        id: message_id,
                        text: delta.to_owned(),
                        attachments: Vec::new(),
                    },
                );
            }
        }
        out
    }

    /// Appends a non-text block to the message stream, creating the stream when
    /// the block arrived without any text (an image-only answer).
    pub(super) fn push_message_attachment(
        &mut self,
        subagent_id: Option<String>,
        message_id: String,
        attachment: AttachmentMeta,
    ) -> Vec<UnifiedEventPayload> {
        let out = self.push_message(subagent_id.clone(), message_id, "");
        if let Some(stream) = self.messages.get_mut(&subagent_id) {
            stream.attachments.push(attachment);
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
            .is_some_and(|stream| stream.id != thought_id)
        {
            out.extend(self.finish_thought(&subagent_id));
        }
        match self.thoughts.get_mut(&subagent_id) {
            Some(stream) => stream.text.push_str(delta),
            None => {
                self.thoughts.insert(
                    subagent_id,
                    PendingStream {
                        id: thought_id,
                        text: delta.to_owned(),
                        attachments: Vec::new(),
                    },
                );
            }
        }
        out
    }

    /// Appends a non-text block to the reasoning stream.
    pub(super) fn push_thought_attachment(
        &mut self,
        subagent_id: Option<String>,
        thought_id: String,
        attachment: AttachmentMeta,
    ) -> Vec<UnifiedEventPayload> {
        let out = self.push_thought(subagent_id.clone(), thought_id, "");
        if let Some(stream) = self.thoughts.get_mut(&subagent_id) {
            stream.attachments.push(attachment);
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
            .is_some_and(|stream| stream.id != message_id)
        {
            out.extend(self.finish_user_message(&subagent_id));
        }
        match self.user_messages.get_mut(&subagent_id) {
            Some(stream) => stream.text.push_str(delta),
            None => {
                self.user_messages.insert(
                    subagent_id,
                    PendingStream {
                        id: message_id,
                        text: delta.to_owned(),
                        attachments: Vec::new(),
                    },
                );
            }
        }
        out
    }

    /// Appends a non-text block to a replayed user message.
    pub(super) fn push_user_message_attachment(
        &mut self,
        subagent_id: Option<String>,
        message_id: String,
        attachment: AttachmentMeta,
    ) -> Vec<UnifiedEventPayload> {
        let out = self.push_user_message(subagent_id.clone(), message_id, "");
        if let Some(stream) = self.user_messages.get_mut(&subagent_id) {
            stream.attachments.push(attachment);
        }
        out
    }

    /// Finalizes every open stream for a single scope in timeline order.
    pub(super) fn finish_scope(
        &mut self,
        subagent_id: &Option<String>,
    ) -> Vec<UnifiedEventPayload> {
        let mut out = Vec::new();
        out.extend(self.finish_user_message(subagent_id));
        // Defensive ordering: reasoning precedes the answer within a turn.
        out.extend(self.finish_thought(subagent_id));
        out.extend(self.finish_message(subagent_id));
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
                let message_id = message_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "acp-user-message".to_owned());
                match chunk_part(&content) {
                    Some(ChunkPart::Text(text)) => {
                        for finished in
                            accumulator.push_user_message(scope.clone(), message_id, &text)
                        {
                            events.push(DriverEvent::Event(finished));
                        }
                    }
                    Some(ChunkPart::Attachment(attachment)) => {
                        for finished in accumulator.push_user_message_attachment(
                            scope.clone(),
                            message_id,
                            attachment,
                        ) {
                            events.push(DriverEvent::Event(finished));
                        }
                    }
                    None => tracing::trace!("ignoring unsupported user message content"),
                }
            } else {
                // User messages are already recorded by the manager as `UserMessage`.
                // The echo marks the start of a turn: a background report that arrived
                // before it (agy's teamwork only flushes its follow-up when the next
                // prompt starts) belongs to the previous context, so finalize it as its
                // own message instead of merging it into this turn's answer.
                for payload in accumulator.finish_scope(&scope) {
                    events.push(DriverEvent::Event(payload));
                }
            }
        }
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content,
            message_id,
            ..
        }) => {
            let message_id = message_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "acp-message".to_owned());
            match chunk_part(&content) {
                Some(ChunkPart::Text(text)) => {
                    for finished in
                        accumulator.push_message(scope.clone(), message_id.clone(), &text)
                    {
                        events.push(DriverEvent::Event(finished));
                    }
                    events.push(DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta {
                        message_id,
                        text_delta: text,
                        subagent_id: scope,
                    }));
                }
                Some(ChunkPart::Attachment(attachment)) => {
                    for finished in
                        accumulator.push_message_attachment(scope.clone(), message_id, attachment)
                    {
                        events.push(DriverEvent::Event(finished));
                    }
                }
                None => tracing::trace!("ignoring unsupported agent message content"),
            }
        }
        SessionUpdate::AgentThoughtChunk(ContentChunk {
            content,
            message_id,
            ..
        }) => {
            let thought_id = message_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "acp-thought".to_owned());
            match chunk_part(&content) {
                Some(ChunkPart::Text(text)) => {
                    for finished in
                        accumulator.push_thought(scope.clone(), thought_id.clone(), &text)
                    {
                        events.push(DriverEvent::Event(finished));
                    }
                    events.push(DriverEvent::Delta(StreamDeltaPayload::AgentThoughtDelta {
                        thought_id,
                        text_delta: text,
                        subagent_id: scope,
                    }));
                }
                Some(ChunkPart::Attachment(attachment)) => {
                    for finished in
                        accumulator.push_thought_attachment(scope.clone(), thought_id, attachment)
                    {
                        events.push(DriverEvent::Event(finished));
                    }
                }
                None => tracing::trace!("ignoring unsupported agent thought content"),
            }
        }
        SessionUpdate::ToolCall(tool_call) => {
            for payload in accumulator.finish_scope(&scope) {
                events.push(DriverEvent::Event(payload));
            }
            events.push(DriverEvent::Event(tool_call_event(&tool_call, subagent_id)));
        }
        SessionUpdate::ToolCallUpdate(update) => {
            events.push(DriverEvent::Event(tool_call_update_event(
                &update,
                subagent_id,
            )));
        }
        SessionUpdate::Plan(plan) => {
            for payload in accumulator.finish_scope(&scope) {
                events.push(DriverEvent::Event(payload));
            }
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
    let (diff, _, text_content, terminal_id) = tool_call_contents(&tool_call.content);
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
        terminal_id,
    }
}

/// Converts an ACP `ToolCallUpdate` into a normalized event (appended to the same `tool_call_id`).
pub(super) fn tool_call_update_event(
    update: &ToolCallUpdate,
    subagent_id: Option<&str>,
) -> UnifiedEventPayload {
    let fields = &update.fields;
    let (diff, locations, text_content, terminal_id) = fields
        .content
        .as_ref()
        .map(|content| tool_call_contents(content))
        .unwrap_or((None, Vec::new(), Vec::new(), None));
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
        terminal_id,
        subagent_id: subagent_id.map(str::to_owned),
    }
}

/// Extracts the diff, file paths, text output and embedded terminal from a
/// `ToolCallContent` list.
pub(super) fn tool_call_contents(
    contents: &[ToolCallContent],
) -> (Option<FileDiff>, Vec<String>, Vec<String>, Option<String>) {
    let mut diff = None;
    let mut locations = Vec::new();
    let mut texts = Vec::new();
    let mut terminal_id = None;
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
            ToolCallContent::Terminal(terminal) => {
                terminal_id = Some(terminal.terminal_id.to_string());
            }
            _ => {}
        }
    }
    (diff, locations, texts, terminal_id)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_for_mime_uses_the_registry() {
        assert_eq!(extension_for_mime("image/png"), "png");
        assert_eq!(extension_for_mime("image/jpeg"), "jpg");
        assert_eq!(extension_for_mime("image/svg+xml"), "svg");
        assert_eq!(extension_for_mime("audio/x-wav"), "wav");
        assert_eq!(extension_for_mime("audio/mpeg"), "mp3");
        assert_eq!(extension_for_mime("application/octet-stream"), "bin");
    }

    #[test]
    fn resource_links_decoded_paths_are_node_local() {
        let attachment =
            inline_attachment("image", "image/png", "aGk=", Some("file:///tmp/a%20b.png"));
        #[cfg(not(windows))]
        assert_eq!(attachment.local_path.as_deref(), Some("/tmp/a b.png"));
        assert_eq!(attachment.file_name, "a b.png");
    }
}
