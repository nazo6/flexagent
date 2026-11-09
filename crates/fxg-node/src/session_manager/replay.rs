use super::*;

impl SessionManager {
    /// Loads the persistent events of a session.
    pub(super) async fn load_session_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionEventEnvelope>, NodeError> {
        let mut cursor = 0u64;
        let mut events = Vec::new();
        loop {
            let batch = self
                .inner
                .bus
                .db()
                .session_events_after(session_id, cursor, EVENT_LOAD_BATCH)
                .await?;
            let full_batch = batch.events.len() as u32 >= EVENT_LOAD_BATCH;
            cursor = batch.cursor;
            events.extend(batch.events);
            if !full_batch {
                return Ok(events);
            }
        }
    }
}

/// Builds the conversation context for an explicit resume replay.
///
/// Concatenates the conversation (user / agent messages) plus tool call summaries
/// and changed files in `node_seq` order (truncated at `up_to_seq`).
/// Returns `None` when there is no conversation (no history).
pub(super) fn build_history_replay_context(
    events: &[SessionEventEnvelope],
    up_to_seq: u64,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for event in events {
        if event.node_seq > up_to_seq {
            break;
        }
        match &event.payload {
            UnifiedEventPayload::UserMessage { text, .. } => {
                parts.push(format!("### User\n{text}"));
            }
            UnifiedEventPayload::AgentMessage {
                text,
                is_complete: true,
                ..
            } => {
                parts.push(format!("### Assistant\n{text}"));
            }
            UnifiedEventPayload::ToolCall {
                title,
                status,
                locations,
                diff,
                ..
            } => {
                let mut line = format!("- [{status}] {title}");
                if !locations.is_empty() {
                    line.push_str(&format!(" ({})", locations.join(", ")));
                }
                if let Some(diff) = diff {
                    line.push_str(&format!(
                        " — {} (+{}/-{})",
                        diff.path, diff.additions, diff.deletions
                    ));
                }
                parts.push(line);
            }
            _ => {}
        }
    }
    assemble_replay_context(parts, up_to_seq)
}

/// Shared assembly of the replay context (truncated at the character limit).
///
/// When the history exceeds the limit, the **most recent** turns are kept
/// (older turns are dropped) so a resumed agent keeps the context closest to
/// the current request.
pub(super) fn assemble_replay_context(parts: Vec<String>, up_to_seq: u64) -> Option<String> {
    /// Maximum number of characters in the injected context (prevents oversized prompts).
    const MAX_CONTEXT_CHARS: usize = 64_000;

    if parts.is_empty() {
        return None;
    }
    let range = if up_to_seq > 0 {
        format!(" (node_seq <= {up_to_seq})")
    } else {
        String::new()
    };
    let header = format!(
        "The history below is from a stopped session{range} that has been resumed. \
         Continue working with this context in mind.\n\n"
    );

    let mut selected: Vec<String> = Vec::new();
    let mut used = header.chars().count();
    for part in parts.iter().rev() {
        let part_chars = part.chars().count() + 2;
        if used + part_chars > MAX_CONTEXT_CHARS && !selected.is_empty() {
            break;
        }
        selected.push(part.clone());
        used += part_chars;
    }
    selected.reverse();
    let truncated = selected.len() < parts.len();

    let mut context = header;
    if truncated {
        context.push_str("[older history omitted because it exceeded the limit]\n\n");
    }
    for part in selected {
        if context.chars().count() + part.chars().count() > MAX_CONTEXT_CHARS {
            let remaining = MAX_CONTEXT_CHARS.saturating_sub(context.chars().count());
            let tail: String = part
                .chars()
                .rev()
                .take(remaining)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            context.push_str(&tail);
            break;
        }
        context.push_str(&part);
        context.push_str("\n\n");
    }
    Some(context)
}
