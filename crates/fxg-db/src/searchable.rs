//! `searchable_text` generation logic.
//!
//! The side that persists events (the receive handlers on both node and hub) uses
//! [`classify_payload`] to generate `session_events.event_type` and
//! `searchable_text` (the FTS5 `tokenize='trigram'` search target).
//!
//! `TerminalOutput` (raw bytes including ANSI escapes), `TerminalInput` (keystrokes),
//! `PermissionResolved` / `SessionAgentBound`, and other binary/identifier events are
//! excluded from FTS (design: `docs/02-database-schema.md` §2).

use fxg_protocol::common::EventType;
use fxg_protocol::events::UnifiedEventPayload;

/// Maximum length (in characters) of text fed into FTS5.
///
/// Prevents the index from bloating on huge tool outputs.
pub const MAX_SEARCHABLE_CHARS: usize = 8 * 1024;

/// Generates (`event_type`, `searchable_text`) from a payload.
///
/// `event_type` is the same vocabulary stored in `session_events.event_type`
/// ([`UnifiedEventPayload::event_type`]).
pub fn classify_payload(payload: &UnifiedEventPayload) -> (EventType, Option<String>) {
    let event_type = payload.event_type();
    let text = match payload {
        UnifiedEventPayload::SessionTitleChanged { title } => join_parts(vec![title.clone()]),
        UnifiedEventPayload::UserMessage {
            text, attachments, ..
        } => {
            let mut parts = vec![text.clone()];
            parts.extend(attachments.iter().map(|a| a.file_name.clone()));
            join_parts(parts)
        }
        UnifiedEventPayload::AgentMessage { text, .. } => join_parts(vec![text.clone()]),
        UnifiedEventPayload::AgentThought { text, .. } => join_parts(vec![text.clone()]),
        UnifiedEventPayload::ToolCall {
            title,
            locations,
            raw_output,
            ..
        } => {
            let mut parts = vec![title.clone()];
            parts.extend(locations.iter().cloned());
            if let Some(output) = raw_output {
                parts.push(output.clone());
            }
            join_parts(parts)
        }
        UnifiedEventPayload::PlanUpdate { entries } => join_parts(
            entries
                .iter()
                .map(|entry| format!("{} {}", entry.title, entry.status))
                .collect(),
        ),
        UnifiedEventPayload::PermissionRequest {
            tool_name, summary, ..
        } => join_parts(vec![tool_name.clone(), summary.clone()]),
        UnifiedEventPayload::ElicitationRequest { message, .. } => {
            join_parts(vec![message.clone()])
        }
        UnifiedEventPayload::CapabilitiesUpdated {
            current_mode,
            available_modes,
            available_commands,
            ..
        } => {
            let mut parts = Vec::new();
            if let Some(mode) = current_mode {
                parts.push(mode.clone());
            }
            parts.extend(available_modes.iter().map(|m| m.mode_id.clone()));
            parts.extend(
                available_commands
                    .iter()
                    .map(|c| format!("{} {}", c.name, c.description)),
            );
            join_parts(parts)
        }
        UnifiedEventPayload::BootstrapLog { line } => join_parts(vec![line.clone()]),
        UnifiedEventPayload::StatusChanged {
            status,
            error_message,
        } => {
            let mut parts = vec![status.to_string()];
            if let Some(message) = error_message {
                parts.push(message.clone());
            }
            join_parts(parts)
        }
        // Not FTS targets (binary, identifier, and projection-only source events)
        UnifiedEventPayload::SessionCreated { .. }
        | UnifiedEventPayload::SessionAgentBound { .. }
        | UnifiedEventPayload::UsageUpdated { .. }
        | UnifiedEventPayload::CompactionUpdated { .. }
        | UnifiedEventPayload::TurnEnded { .. }
        | UnifiedEventPayload::PermissionResolved { .. }
        | UnifiedEventPayload::ElicitationResolved { .. }
        | UnifiedEventPayload::SessionReverted { .. }
        | UnifiedEventPayload::SessionArchived { .. }
        | UnifiedEventPayload::SessionDeleted {}
        | UnifiedEventPayload::TerminalOutput { .. }
        | UnifiedEventPayload::TerminalInput { .. } => None,
    };

    (
        event_type,
        text.map(|t| truncate_chars(&t, MAX_SEARCHABLE_CHARS)),
    )
}

/// Joins non-empty parts with newlines. Returns `None` if every part is empty.
fn join_parts(parts: Vec<String>) -> Option<String> {
    let joined = parts
        .into_iter()
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// Truncates to `max_chars` characters without breaking character boundaries.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::common::{
        AttachmentMeta, ClientSource, EventType, FileDiff, PermissionOption, PermissionOptionKind,
        PlanEntry, PlanEntryStatus, ToolCallKind, ToolCallStatus,
    };

    #[test]
    fn user_message_includes_text_and_attachment_names() {
        let payload = UnifiedEventPayload::UserMessage {
            text: "Fix the authentication error".to_owned(),
            attachments: vec![AttachmentMeta {
                file_name: "screenshot.png".to_owned(),
                mime_type: "image/png".to_owned(),
                size_bytes: 10,
                local_path: None,
                data_b64: None,
            }],
            client_source: ClientSource::Cli,
            snapshot_tree_hash: None,
        };
        let (event_type, text) = classify_payload(&payload);
        assert_eq!(event_type, EventType::UserMessage);
        let text = text.unwrap();
        assert!(text.contains("Fix the authentication error"));
        assert!(text.contains("screenshot.png"));
    }

    #[test]
    fn tool_call_includes_title_locations_and_output() {
        let payload = UnifiedEventPayload::ToolCall {
            tool_call_id: "t1".to_owned(),
            title: "cargo test --workspace".to_owned(),
            kind: ToolCallKind::Execute,
            status: ToolCallStatus::Completed,
            locations: vec!["crates/fxg-db/src/lib.rs".to_owned()],
            diff: Some(FileDiff {
                path: "a.rs".to_owned(),
                old_text: None,
                new_text: None,
                unified_diff: "diff".to_owned(),
                additions: 1,
                deletions: 0,
            }),
            raw_output: Some("test result: FAILED".to_owned()),
        };
        let (event_type, text) = classify_payload(&payload);
        assert_eq!(event_type, EventType::ToolCall);
        let text = text.unwrap();
        assert!(text.contains("cargo test --workspace"));
        assert!(text.contains("crates/fxg-db/src/lib.rs"));
        assert!(text.contains("test result: FAILED"));
    }

    #[test]
    fn terminal_output_and_input_are_excluded_from_fts() {
        let output = UnifiedEventPayload::TerminalOutput {
            terminal_id: "t1".to_owned(),
            command: "ls".to_owned(),
            data_b64: "GVsbG8=".to_owned(),
            exit_code: None,
        };
        assert_eq!(classify_payload(&output).1, None);

        let input = UnifiedEventPayload::TerminalInput {
            terminal_id: "t1".to_owned(),
            data_b64: "aGk=".to_owned(),
        };
        assert_eq!(classify_payload(&input).1, None);
    }

    #[test]
    fn permission_request_is_searchable() {
        let payload = UnifiedEventPayload::PermissionRequest {
            request_id: "r1".to_owned(),
            tool_name: "terminal/create".to_owned(),
            summary: "Run command: cargo test --workspace".to_owned(),
            options: vec![PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow".to_owned(),
                kind: PermissionOptionKind::AllowOnce,
            }],
            details: serde_json::json!({}),
        };
        let (event_type, text) = classify_payload(&payload);
        assert_eq!(event_type, EventType::PermissionRequest);
        assert!(text.unwrap().contains("cargo test --workspace"));
    }

    #[test]
    fn plan_and_status_are_searchable() {
        let plan = UnifiedEventPayload::PlanUpdate {
            entries: vec![PlanEntry {
                id: "1".to_owned(),
                title: "Add tests".to_owned(),
                status: PlanEntryStatus::InProgress,
            }],
        };
        assert_eq!(classify_payload(&plan).0, EventType::Plan);
        assert!(classify_payload(&plan).1.unwrap().contains("Add tests"));

        let status = UnifiedEventPayload::StatusChanged {
            status: fxg_protocol::common::SessionStatus::Error,
            error_message: Some("Authentication token is invalid".to_owned()),
        };
        assert_eq!(classify_payload(&status).0, EventType::StatusChanged);
        assert!(
            classify_payload(&status)
                .1
                .unwrap()
                .contains("Authentication token is invalid")
        );
    }

    #[test]
    fn session_created_has_no_searchable_text() {
        let payload = UnifiedEventPayload::SessionCreated {
            node_id: "n".to_owned(),
            project_id: "github.com/nazo6/flexagent".to_owned(),
            project_name: "flexagent".to_owned(),
            local_path: "/tmp".to_owned(),
            git_branch: None,
            is_worktree: false,
            agent_id: "opencode".to_owned(),
            parent_session_id: None,
            fork_from_node_seq: None,
            title: "New Session".to_owned(),
        };
        assert_eq!(classify_payload(&payload).1, None);
    }

    #[test]
    fn long_text_is_truncated_on_char_boundary() {
        let long = "あ".repeat(MAX_SEARCHABLE_CHARS + 100); // allow-japanese: multibyte truncation test
        let payload = UnifiedEventPayload::AgentMessage {
            message_id: "m".to_owned(),
            text: long,
            is_complete: true,
        };
        let text = classify_payload(&payload).1.unwrap();
        assert_eq!(text.chars().count(), MAX_SEARCHABLE_CHARS + 1); // + ellipsis character
        assert!(text.ends_with('…'));
    }

    #[test]
    fn empty_texts_collapse_to_none() {
        let payload = UnifiedEventPayload::AgentMessage {
            message_id: "m".to_owned(),
            text: "   ".to_owned(),
            is_complete: true,
        };
        assert_eq!(classify_payload(&payload).1, None);
    }
}
